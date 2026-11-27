use crate::durable::{CanonicalStore, JournalStore};
mod agents;
mod authored_paths;
mod compiler_models;
mod data_nodes;
mod graph;
#[cfg(unix)]
#[path = "tests/model.rs"]
mod model_tests;
#[cfg(unix)]
mod publication_tests;
mod scoped;
mod templates;
mod tools;
use super::*;
use serde_json::json;
use std::sync::{Arc, Barrier};
use std::thread;

#[cfg(unix)]
use crate::capabilities::CapabilitySnapshot;

#[cfg(unix)]
#[cfg(unix)]
use crate::model::catalog::{MapCredentialEnvironment, ModelCatalog, ModelRef};
#[cfg(unix)]
use crate::model::invocation::ModelBindingRegistry;
#[cfg(unix)]
use crate::model::session::SessionModelConfig;
#[cfg(unix)]
use crate::runtime::cancellation::{CancellationSignal, ExecutionCancellation};
#[cfg(unix)]
use crate::runtime::identity::{
    AgentId, CapabilityRevision, ConversationId, RuntimeResourceRevision,
};
#[cfg(unix)]
use crate::runtime::subagent::process::StagedChild;
#[cfg(unix)]
use crate::runtime::subagent::{
    NamedAgentDefinition, SubagentRegistry, SubagentRegistryConfig, SubagentSpawnPlan,
};
#[cfg(unix)]
use crate::runtime::types::{ApprovalMode, CancellationReason, SystemClock};
#[cfg(unix)]
use crate::skills::SkillSnapshot;
#[cfg(unix)]
use crate::tools::environment::ToolEnvironment;
#[cfg(unix)]
use crate::tools::executor::ToolRegistry;
#[cfg(unix)]
use crate::tools::mcp::McpRuntimeLeaseAuthority;

/// An End returning these fields of `node`, each under its own name.
fn returns(node: &str, fields: &[&str]) -> Value {
    let outputs = fields
        .iter()
        .map(|field| json!({"variable":field,"value_selector":[node,field]}))
        .collect::<Vec<_>>();
    json!({"type":"return","outputs":outputs})
}

#[allow(clippy::needless_pass_by_value)]
fn schema(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn profile(name: &str) -> SubagentName {
    SubagentName::parse(name).expect("profile")
}

fn reference(path: &str) -> WorkflowValue {
    WorkflowValue::Reference {
        path: path.split('.').map(str::to_owned).collect(),
    }
}

/// A Model prompt: one fixed system message, then one user turn.
fn prompt(system: &str, user: Vec<WorkflowValue>) -> Vec<WorkflowModelMessage> {
    vec![
        WorkflowModelMessage {
            role: WorkflowModelRole::System,
            content: vec![text(system)],
        },
        WorkflowModelMessage {
            role: WorkflowModelRole::User,
            content: user,
        },
    ]
}

fn text(fixed: &str) -> WorkflowValue {
    WorkflowValue::Literal {
        value: Value::String(fixed.to_owned()),
    }
}

fn agent(output: Value) -> WorkflowNodeDefinition {
    WorkflowNodeDefinition::Agent {
        profile: "reviewer".into(),
        task: vec![text("Review the input."), reference("args.task")],
        output: WorkflowAgentOutput::JsonSchema { schema: output },
        routes: Vec::new(),
        permissions: None,
        on_error: WorkflowValueErrorStrategy::default(),
    }
}

fn return_node(output: BTreeMap<String, WorkflowValue>) -> WorkflowNodeDefinition {
    let outputs = output
        .into_iter()
        .map(|(variable, value)| {
            let WorkflowValue::Reference { path } = value else {
                panic!("an End output reads a variable")
            };
            WorkflowReturnOutput {
                variable,
                value_selector: path,
            }
        })
        .collect();
    WorkflowNodeDefinition::Return { outputs }
}

fn edge(from: &str, to: &str) -> WorkflowEdgeDefinition {
    WorkflowEdgeDefinition {
        from: from.to_owned(),
        to: to.to_owned(),
        port: None,
    }
}

fn branch_edge(from: &str, to: &str, port: WorkflowPort) -> WorkflowEdgeDefinition {
    WorkflowEdgeDefinition {
        from: from.to_owned(),
        to: to.to_owned(),
        port: Some(port),
    }
}

fn base_definition(
    entry: &str,
    nodes: BTreeMap<String, WorkflowNodeDefinition>,
    edges: Vec<WorkflowEdgeDefinition>,
) -> WorkflowDefinition {
    WorkflowDefinition {
        description: "Test workflow".to_owned(),
        timeout_ms: 600_000,
        permissions: None,
        block: WorkflowBlock {
            input: schema(json!({"task": {"type": "string"}}), &["task"]),
            nodes: {
                let mut nodes: std::collections::BTreeMap<
                    String,
                    crate::runtime::workflow::WorkflowNodeDefinition,
                > = nodes;
                nodes.insert(
                    "workflow_start".into(),
                    crate::runtime::workflow::WorkflowNodeDefinition::Start {},
                );
                nodes
            },
            edges: {
                let mut edges = edges;
                edges.insert(
                    0,
                    crate::runtime::workflow::WorkflowEdgeDefinition {
                        from: "workflow_start".into(),
                        to: entry.to_owned(),
                        port: None,
                    },
                );
                edges
            },
        },
    }
}

fn compile_test(definition: WorkflowDefinition) -> Result<WorkflowProgram, WorkflowCompileError> {
    WorkflowProgram::compile(WorkflowId::parse("test_workflow").expect("id"), definition)
}

#[cfg(unix)]
const WORKFLOW_TEST_MODELS: &str = r#"[providers.local]
protocol = "openai_chat_completions"
base_url = "http://127.0.0.1:9/v1"
api_key = "test-only-secret"

[models."local/model"]
provider = "local"
id = "model"
context_window = 128000
max_output_tokens = 512

[models."local/model".capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
[models."local/model".compat]
chat_reasoning_replay = "omit"
"#;

#[cfg(unix)]
struct WorkflowTestPlane {
    dir: tempfile::TempDir,
    registry: SubagentRegistry,
    store: Arc<crate::durable::SqliteConversationStore>,
    conversation_id: ConversationId,
    runtime_root: std::path::PathBuf,
}

#[cfg(unix)]
fn workflow_test_plane(max_active: usize) -> WorkflowTestPlane {
    let dir = tempfile::tempdir().expect("workflow test temp dir");
    let workspace = dir.path().join("workspace");
    let runtime_root = dir.path().join("subagents");
    std::fs::create_dir_all(&workspace).expect("workflow workspace");
    std::fs::create_dir_all(&runtime_root).expect("workflow runtime root");
    let conversation_id = ConversationId::new("conv_fb777980-3e94-78c5-879a-296623a08f02");
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(conversation_id.clone())
            .expect("workflow store"),
    );
    workflow_test_plane_with_store(dir, conversation_id, store, max_active)
}

#[cfg(unix)]
fn workflow_test_plane_with_store(
    dir: tempfile::TempDir,
    conversation_id: ConversationId,
    store: Arc<crate::durable::SqliteConversationStore>,
    max_active: usize,
) -> WorkflowTestPlane {
    let workspace = dir.path().join("workspace");
    let runtime_root = dir.path().join("subagents");
    let mailbox = crate::runtime::inbound::ConversationInboundMailbox::over_store(store.clone());
    let registry = SubagentRegistry::new(SubagentRegistryConfig {
        conversation_id: conversation_id.clone(),
        agent_id: AgentId::new("workflow-test-parent"),
        mailbox,
        clock: Arc::new(SystemClock),
        monotonic_clock: Arc::new(crate::runtime::ManualMonotonicClock::new()),
        spawn: SubagentSpawnPlan {
            session_id: crate::runtime::identity::SessionId::new(
                "ses_01900000-0000-7000-8000-000000000001",
            ),

            program: std::path::PathBuf::from("/nonexistent/rustx"),
            product_root: crate::runtime::local_storage::ProductRoot::create(&runtime_root.clone())
                .expect("product root"),
        },
        workspace: workspace.clone(),
        max_active,
    });
    WorkflowTestPlane {
        dir,
        registry,
        store,
        conversation_id,
        runtime_root,
    }
}

#[cfg(unix)]
fn workflow_test_context(
    plane: &WorkflowTestPlane,
) -> crate::runtime::subagent::NativeExecutionContext {
    workflow_test_context_for_generation(
        plane,
        1,
        "Workflow test instructions",
        WorkflowCatalog::empty(),
        ApprovalMode::FullAccess,
        None,
    )
}

#[cfg(unix)]
fn workflow_test_context_for_generation(
    plane: &WorkflowTestPlane,
    revision: u64,
    instructions: &str,
    workflow_catalog: WorkflowCatalog,
    approval_mode: ApprovalMode,
    reviewer_permissions: Option<crate::runtime::permission::ResourcePermissionSet>,
) -> crate::runtime::subagent::NativeExecutionContext {
    let model_catalog = ModelCatalog::from_toml_slice(WORKFLOW_TEST_MODELS.as_bytes())
        .expect("workflow test model catalog");
    let models = ModelBindingRegistry::new(
        model_catalog
            .resolve(&MapCredentialEnvironment::default())
            .expect("workflow test model resolution"),
    )
    .expect("workflow test model bindings");
    let model = ModelRef::parse("local/model").expect("workflow test model");
    let reviewer = profile("reviewer");
    let definition = NamedAgentDefinition::new(
        reviewer.clone(),
        crate::runtime::agent_profile::AgentProfile {
            description: "Workflow test reviewer".to_owned(),
            instructions: instructions.to_owned(),
            model: Some(SessionModelConfig::of(model.clone())),
            execution_deadline: None,
            tools: Vec::new(),
            skills: crate::runtime::agent_profile::AgentSkillSelection::default(),
            inherit_project_instructions: false,
            extensions: crate::extensions::NativeAgentExtensionsDocument::default().resolve(),
            workflows: std::collections::BTreeSet::default(),
            permissions: reviewer_permissions,
        },
        plane.dir.path().join("reviewer.md"),
    )
    .expect("workflow test subagent definition");
    let catalog = crate::runtime::subagent::AgentCatalog::new([definition])
        .expect("workflow test subagent catalog");
    let capabilities = Arc::new(CapabilitySnapshot::new(
        plane.conversation_id.clone(),
        plane.dir.path().join("workspace"),
        CapabilityRevision::new(1),
        Arc::new(ToolRegistry::new()),
        Arc::new(crate::capabilities::AvailableToolCatalog::default()),
        Arc::new(SkillSnapshot::new(Vec::new())),
        ToolEnvironment::new(),
        Arc::new(McpRuntimeLeaseAuthority::empty()),
        Arc::new(std::collections::BTreeMap::new()),
    ));
    let resources = Arc::new(
        crate::runtime::RuntimeResourceSnapshot::new(
            RuntimeResourceRevision::new(revision),
            Vec::new(),
            None,
            crate::context::ContextAssembly::new(),
            capabilities,
        )
        .with_subagent_catalog(catalog)
        .with_workflow_catalog(workflow_catalog),
    );
    let mut context = crate::runtime::subagent::NativeExecutionContext::test_context(
        crate::runtime::identity::AttemptId::new("workflow-test-attempt"),
        resources,
        SessionModelConfig::of(model),
        models,
        approval_mode,
    );
    let attempt_id = context.attempt_id().expect("workflow test attempt").clone();
    context.native = Some(std::sync::Arc::new(
        crate::tools::invocation::NativeInvocationServices {
            task: None,
            lifecycle: crate::agent::AttemptLifecycle::default().with_approval_mode(approval_mode),
            runtime: crate::tools::runtime::ConversationToolRuntime::new(
                plane.conversation_id.clone(),
                plane.dir.path().join("workspace"),
                plane.dir.path().join("artifacts"),
            )
            .expect("workflow test tool runtime"),
            clock: std::sync::Arc::new(crate::runtime::ManualMonotonicClock::new()),
            leaf_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
            execution: crate::runtime::interaction::InteractionExecution::Attempt {
                attempt_id,
                turn: 1,
            },
            scheduling: std::sync::Arc::new(tokio::sync::RwLock::new(())),
            descendants: crate::tools::invocation::NativeChildScope::default(),
        },
    ));
    context
}

#[cfg(unix)]
struct ScriptedWorkflowChild {
    peer: tokio::net::UnixStream,
    root: std::path::PathBuf,
}

#[cfg(unix)]
fn stage_workflow_child(plane: &WorkflowTestPlane) -> ScriptedWorkflowChild {
    let (driver_end, test_end) = tokio::net::UnixStream::pair().expect("workflow IPC pair");
    let (observation_end, _observation_peer) =
        tokio::net::UnixStream::pair().expect("observation pair");
    let child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg("true")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .expect("workflow scripted child");
    let pid = child.id().expect("workflow scripted child pid");
    let root = plane.runtime_root.join(format!("test-child-{pid}"));
    std::fs::create_dir_all(&root).expect("workflow child runtime root");
    plane.registry.push_staged_override(StagedChild::for_test(
        child,
        driver_end,
        observation_end,
        root.clone(),
    ));
    ScriptedWorkflowChild {
        peer: test_end,
        root,
    }
}

#[cfg(unix)]
impl ScriptedWorkflowChild {
    async fn expect_delegate(&mut self) {
        let frame = crate::runtime::subagent::ipc::read_parent_frame(&mut self.peer)
            .await
            .expect("workflow delegate frame");
        assert!(matches!(
            frame,
            Some(crate::runtime::subagent::ipc::ParentFrame::Delegate(_))
        ));
    }

    async fn send_result(
        &mut self,
        status: crate::runtime::subagent::ipc::ChildResultStatus,
        content: Option<&str>,
    ) {
        crate::runtime::subagent::ipc::write_child_frame(
            &mut self.peer,
            &crate::runtime::subagent::ipc::ChildFrame::Result(
                crate::runtime::subagent::ipc::ResultFrame {
                    status,
                    content: content.map(str::to_owned),
                    diagnostic: None,
                },
            ),
        )
        .await
        .expect("workflow child result frame");
    }

    async fn cancel_after_delegate(&mut self) {
        let frame = crate::runtime::subagent::ipc::read_parent_frame(&mut self.peer)
            .await
            .expect("workflow cancel frame");
        assert!(matches!(
            frame,
            Some(crate::runtime::subagent::ipc::ParentFrame::Cancel {
                reason: Some(CancellationReason::UserRequested)
            })
        ));
        crate::runtime::subagent::ipc::write_child_frame(
            &mut self.peer,
            &crate::runtime::subagent::ipc::ChildFrame::Result(
                crate::runtime::subagent::ipc::ResultFrame {
                    status: crate::runtime::subagent::ipc::ChildResultStatus::Cancelled,
                    content: None,
                    diagnostic: None,
                },
            ),
        )
        .await
        .expect("workflow cancellation result frame");
    }
}

#[cfg(unix)]
fn parallel_test_program(keys: &[&str]) -> Arc<WorkflowProgram> {
    let output = schema(json!({"summary":{"type":"string"}}), &["summary"]);
    let mut nodes = BTreeMap::from([("workflow_start".into(), WorkflowNodeDefinition::Start {})]);
    let mut edges = Vec::new();
    let mut fields = BTreeMap::new();
    let mut properties = serde_json::Map::new();
    for key in keys {
        nodes.insert(
            (*key).into(),
            WorkflowNodeDefinition::Agent {
                profile: "reviewer".into(),
                task: vec![text(&format!("Run the {key} workflow branch."))],
                output: WorkflowAgentOutput::JsonSchema {
                    schema: output.clone(),
                },
                routes: Vec::new(),
                permissions: None,
                on_error: WorkflowValueErrorStrategy::default(),
            },
        );
        edges.extend([edge("workflow_start", key), edge(key, "done")]);
        fields.insert((*key).into(), reference(key));
        properties.insert((*key).into(), output.clone());
    }
    nodes.insert("done".into(), return_node(fields));
    Arc::new(
        compile_test(WorkflowDefinition {
            description: "Sibling fanout".into(),
            timeout_ms: 600_000,
            permissions: None,
            block: WorkflowBlock {
                input: schema(json!({"task":{"type":"string"}}), &["task"]),
                nodes,
                edges,
            },
        })
        .unwrap(),
    )
}

#[cfg(unix)]
fn snapshot_test_program(result_field: &str, description: &str) -> Arc<WorkflowProgram> {
    let output = schema(json!({"summary":{"type":"string"}}), &["summary"]);
    let mut definition = base_definition(
        "work",
        BTreeMap::from([
            (
                "work".into(),
                WorkflowNodeDefinition::Agent {
                    profile: "reviewer".into(),
                    task: vec![text(&format!("Run the {description} branch."))],
                    output: WorkflowAgentOutput::JsonSchema {
                        schema: output.clone(),
                    },
                    routes: Vec::new(),
                    permissions: None,
                    on_error: WorkflowValueErrorStrategy::default(),
                },
            ),
            (
                "done".into(),
                return_node(BTreeMap::from([(result_field.into(), reference("work"))])),
            ),
        ]),
        vec![edge("work", "done")],
    );
    definition.description = description.into();
    definition.block.input = schema(json!({}), &[]);
    Arc::new(
        WorkflowProgram::compile(WorkflowId::parse("snapshot_workflow").unwrap(), definition)
            .unwrap(),
    )
}

#[cfg(unix)]
fn return_only_program() -> Arc<WorkflowProgram> {
    let output = schema(json!({"value": {"type": "string"}}), &["value"]);
    Arc::new(
        WorkflowProgram::compile(
            WorkflowId::parse("event_journal_workflow").expect("event workflow id"),
            WorkflowDefinition {
                timeout_ms: 600_000,
                permissions: None,
                description: "Event journal test workflow".to_owned(),
                block: WorkflowBlock {
                    input: output.clone(),
                    nodes: {
                        let mut nodes: std::collections::BTreeMap<
                            String,
                            crate::runtime::workflow::WorkflowNodeDefinition,
                        > = BTreeMap::from([(
                            "done".to_owned(),
                            return_node(BTreeMap::from([(
                                "value".to_owned(),
                                reference("args.value"),
                            )])),
                        )]);
                        nodes.insert(
                            "workflow_start".into(),
                            crate::runtime::workflow::WorkflowNodeDefinition::Start {},
                        );
                        nodes
                    },
                    edges: {
                        let mut edges = Vec::new();
                        edges.insert(
                            0,
                            crate::runtime::workflow::WorkflowEdgeDefinition {
                                from: "workflow_start".into(),
                                to: "done".to_owned(),
                                port: None,
                            },
                        );
                        edges
                    },
                },
            },
        )
        .expect("return-only workflow program"),
    )
}

/// Delegate is the gate: ownership has committed, while the physical
/// driver cannot settle until this test sends its result. Inject the exact
/// cleanup or durable-commit failure on that side of the gate, then consume
/// the real Workflow result and the registry's actual settlement snapshot.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // Two real settlement dimensions share the same gated Workflow fixture.
async fn workflow_distinguishes_committed_physical_failure_from_abandoned_publication() {
    use crate::runtime::subagent::{SubagentPhysicalSettlement, SubagentPublication};

    for abandon_publication in [false, true] {
        let plane = workflow_test_plane(1);
        let mut child = stage_workflow_child(&plane);
        let runtime = workflow_runtime(&plane);
        let context = workflow_test_context(&plane);
        let output = schema(json!({"summary": {"type": "string"}}), &["summary"]);
        let program = Arc::new(
            compile_test(base_definition(
                "work",
                BTreeMap::from([
                    ("work".into(), agent(output.clone())),
                    (
                        "done".into(),
                        return_node(BTreeMap::from([(
                            "summary".to_owned(),
                            reference("work.summary"),
                        )])),
                    ),
                ]),
                vec![edge("work", "done")],
            ))
            .unwrap(),
        );
        let (_, cancellation) = workflow_cancellation();
        let task = tokio::spawn(async move {
            runtime
                .run_test_foreground(
                    program,
                    ToolCallId::new("settlement-dimensions"),
                    context,
                    json!({"task": "produce valid output"}),
                    cancellation,
                )
                .await
        });
        child.expect_delegate().await;
        let running = plane.registry.all_snapshots().pop().unwrap();
        assert!(!running.state.is_terminal());
        assert_eq!(running.settlement.publication, SubagentPublication::Pending);
        assert_eq!(
            running.settlement.physical,
            SubagentPhysicalSettlement::Unproven
        );
        if abandon_publication {
            // This store fault occurs inside the atomic terminal/output
            // transaction. Cover all bounded retries; observability writes
            // cannot use up the failure budget before terminal publication.
            plane.store.arm_fail_event_times(100);
        } else {
            // Replace the exact disposable incarnation root with a file.
            // The native driver's remove_dir_all must fail, after reap;
            // the valid child result alone cannot prove physical cleanup.
            std::fs::remove_dir_all(&child.root).unwrap();
            std::fs::write(&child.root, b"cleanup proof unavailable").unwrap();
        }
        child
            .send_result(
                crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
                Some(r#"{"summary":"valid output"}"#),
            )
            .await;
        let error = task.await.unwrap().unwrap_err();
        let expected = if abandon_publication {
            WorkflowChildSettlementFailure::PublicationAbandoned
        } else {
            WorkflowChildSettlementFailure::PhysicalContainmentUnproven
        };
        assert!(
            matches!(&error, WorkflowRunError::ChildSettlement { failure, .. } if *failure == expected)
        );
        assert!(matches!(
            error.execution_status(),
            crate::tools::types::ToolExecutionStatus::OutcomeUnknown { .. }
        ));
        let snapshot = plane.registry.all_snapshots().pop().unwrap();
        assert!(!snapshot.is_settled());
        let events = plane.store.read_events(None, 128).unwrap().events;
        let committed_terminal = events
            .iter()
            .any(|event| matches!(event.event, RuntimeEvent::SubagentTerminalSettled { .. }));
        if abandon_publication {
            assert_eq!(
                snapshot.settlement.publication,
                SubagentPublication::Abandoned
            );
            assert_eq!(
                snapshot.settlement.physical,
                SubagentPhysicalSettlement::Proven
            );
            assert!(!snapshot.state.is_terminal());
            assert!(!committed_terminal);
            assert!(!child.root.exists());
            assert!(
                error
                    .to_string()
                    .contains("abandoned after durability failure")
            );
        } else {
            assert_eq!(
                snapshot.settlement.publication,
                SubagentPublication::Committed
            );
            assert_eq!(
                snapshot.settlement.physical,
                SubagentPhysicalSettlement::Unproven
            );
            assert!(snapshot.state.is_terminal());
            assert!(committed_terminal);
            assert!(
                error
                    .to_string()
                    .contains("publication committed but physical containment is unproven")
            );
            assert!(!error.to_string().contains("publication did not settle"));
            std::fs::remove_file(&child.root).unwrap();
        }
        assert!(
            !events.iter().any(|event| matches!(
                event.event,
                RuntimeEvent::WorkflowAgentOutputCommitted { .. }
            )),
            "neither unresolved boundary publishes a usable Workflow success"
        );
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_runtime_keys_results_by_definition_when_completion_is_reversed() {
    let plane = workflow_test_plane(2);
    let mut alpha = stage_workflow_child(&plane);
    let mut zulu = stage_workflow_child(&plane);
    let runtime = workflow_runtime(&plane);
    let native = runtime.read_model.clone();
    let mut changed = native.subscribe();
    let context = workflow_test_context(&plane);
    let program = parallel_test_program(&["alpha", "zulu"]);
    let (_, cancellation) = workflow_cancellation();
    let (result_tx, mut result_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let result = runtime
            .run_test_foreground(
                program,
                ToolCallId::new("parallel-reversed"),
                context,
                json!({"task": "parallel"}),
                cancellation,
            )
            .await;
        result_tx
            .send(result.clone())
            .expect("workflow result receiver");
        result
    });

    // Admission is definition-key ordered, but both children are owned
    // before settlement begins. The test holds both Delegate frames so
    // physical result order is controlled independently of that order.
    alpha.expect_delegate().await;
    zulu.expect_delegate().await;
    loop {
        let view = native.snapshot();
        if view.runs[0]
            .instances
            .iter()
            .filter(|row| row.child.is_some())
            .count()
            == 2
        {
            break;
        }
        changed.changed().await.unwrap();
    }
    let before = native.snapshot();
    let children = plane.registry.all_snapshots().len();
    let history = plane.store.load_canonical().unwrap();
    for _ in 0..20 {
        assert_eq!(native.snapshot(), before);
    }
    assert_eq!(plane.registry.all_snapshots().len(), children);
    assert_eq!(plane.store.load_canonical().unwrap(), history);
    let branches: BTreeSet<_> = before.runs[0]
        .instances
        .iter()
        .filter(|row| row.child.is_some())
        .map(|row| (row.block.clone(), row.node.clone()))
        .collect();
    assert_eq!(
        branches.len(),
        2,
        "concurrent concrete branches are independent"
    );
    zulu.send_result(
        crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
        Some(r#"{"summary":"zulu"}"#),
    )
    .await;
    let zulu_id = SubagentId::for_conversation(&plane.conversation_id, 2);
    assert_eq!(
        plane
            .registry
            .wait_until_settled(&zulu_id)
            .await
            .expect("zulu settles first")
            .state,
        crate::runtime::subagent::SubagentState::Succeeded
    );
    assert!(matches!(
        result_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));

    alpha
        .send_result(
            crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
            Some(r#"{"summary":"alpha"}"#),
        )
        .await;
    let result = task
        .await
        .expect("workflow task")
        .expect("workflow success");
    assert_eq!(
        result,
        json!({
            "alpha": {"summary": "alpha"},
            "zulu": {"summary": "zulu"}
        })
    );
    assert_eq!(plane.registry.unsettled_snapshot(), Vec::new());
    assert!(!alpha.root.exists());
    assert!(!zulu.root.exists());
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_runtime_all_settles_multiple_failures_in_definition_order() {
    let plane = workflow_test_plane(2);
    let mut alpha = stage_workflow_child(&plane);
    let mut zulu = stage_workflow_child(&plane);
    let runtime = workflow_runtime(&plane);
    let context = workflow_test_context(&plane);
    let program = parallel_test_program(&["alpha", "zulu"]);
    let (_, cancellation) = workflow_cancellation();
    let (result_tx, mut result_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let result = runtime
            .run_test_foreground(
                program,
                ToolCallId::new("parallel-failures"),
                context,
                json!({"task": "parallel"}),
                cancellation,
            )
            .await;
        result_tx
            .send(result.clone())
            .expect("workflow result receiver");
        result
    });

    alpha.expect_delegate().await;
    zulu.expect_delegate().await;
    zulu.send_result(
        crate::runtime::subagent::ipc::ChildResultStatus::Failed,
        None,
    )
    .await;
    let zulu_id = SubagentId::for_conversation(&plane.conversation_id, 2);
    assert_eq!(
        plane
            .registry
            .wait_until_settled(&zulu_id)
            .await
            .expect("zulu failure settles first")
            .state,
        crate::runtime::subagent::SubagentState::Failed
    );
    assert!(matches!(
        result_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));

    alpha
        .send_result(
            crate::runtime::subagent::ipc::ChildResultStatus::Failed,
            None,
        )
        .await;
    let error = task
        .await
        .expect("workflow task")
        .expect_err("parallel failure");
    let detail = error.to_string();
    let WorkflowRunError::NodesFailed { .. } = error else {
        panic!("expected keyed parallel failure");
    };
    assert!(
        detail.find("alpha").expect("alpha diagnostic")
            < detail.find("zulu").expect("zulu diagnostic")
    );
    assert_eq!(plane.registry.unsettled_snapshot(), Vec::new());
    assert!(!alpha.root.exists());
    assert!(!zulu.root.exists());
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_runtime_capacity_one_waits_and_releases_for_every_branch() {
    let plane = workflow_test_plane(1);
    let mut alpha = stage_workflow_child(&plane);
    let mut zulu = stage_workflow_child(&plane);
    let runtime = workflow_runtime(&plane);
    let context = workflow_test_context(&plane);
    let program = parallel_test_program(&["alpha", "zulu"]);
    let (_, cancellation) = workflow_cancellation();
    let task = tokio::spawn(async move {
        runtime
            .run_test_foreground(
                program,
                ToolCallId::new("parallel-capacity"),
                context,
                json!({"task": "parallel"}),
                cancellation,
            )
            .await
    });

    alpha.expect_delegate().await;
    alpha
        .send_result(
            crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
            Some(r#"{"summary":"alpha"}"#),
        )
        .await;
    zulu.expect_delegate().await;
    zulu.send_result(
        crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
        Some(r#"{"summary":"zulu"}"#),
    )
    .await;
    assert_eq!(
        task.await
            .expect("workflow task")
            .expect("capacity waiter progresses"),
        json!({"alpha":{"summary":"alpha"},"zulu":{"summary":"zulu"}})
    );
    assert_eq!(plane.registry.all_snapshots().len(), 2);
    assert_eq!(plane.registry.unsettled_snapshot(), Vec::new());
    assert!(!alpha.root.exists());
    assert!(!zulu.root.exists());
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_runtime_cancellation_drains_every_admitted_child() {
    let plane = workflow_test_plane(2);
    let mut alpha = stage_workflow_child(&plane);
    let mut zulu = stage_workflow_child(&plane);
    let runtime = workflow_runtime(&plane);
    let views = runtime.read_model.clone();
    let mut changes = views.subscribe();
    let context = workflow_test_context(&plane);
    let program = parallel_test_program(&["alpha", "zulu"]);
    let (signal, cancellation) = workflow_cancellation();
    let task = tokio::spawn(async move {
        runtime
            .run_test_foreground(
                program,
                ToolCallId::new("parallel-cancel"),
                context,
                json!({"task": "parallel"}),
                cancellation,
            )
            .await
    });

    alpha.expect_delegate().await;
    zulu.expect_delegate().await;
    signal.cancel();
    loop {
        if matches!(
            views.snapshot().runs[0].state,
            read_model::WorkflowState::Draining
        ) {
            break;
        }
        changes.changed().await.unwrap();
    }
    signal.cancel();
    assert_eq!(
        plane.registry.all_snapshots().len(),
        2,
        "cancellation never admits more branches"
    );
    alpha.cancel_after_delegate().await;
    zulu.cancel_after_delegate().await;

    let error = task
        .await
        .expect("workflow task")
        .expect_err("workflow cancellation");
    assert!(error.is_cancelled());
    let cuts = views.cuts_after(read_model::WorkflowRevision(0));
    assert_eq!(
        cuts.iter()
            .filter(|cut| matches!(cut.runs[0].state, read_model::WorkflowState::Settled { .. }))
            .count(),
        1
    );
    assert!(matches!(
        views.snapshot().runs[0].state,
        read_model::WorkflowState::Settled {
            outcome: WorkflowExecutionOutcome::Cancelled
        }
    ));
    let snapshots = plane.registry.all_snapshots();
    assert_eq!(snapshots.len(), 2);
    assert!(snapshots.iter().all(|snapshot| {
        snapshot.state == crate::runtime::subagent::SubagentState::Cancelled
            && snapshot.is_settled()
    }));
    assert_eq!(plane.registry.unsettled_snapshot(), Vec::new());
    assert!(!alpha.root.exists());
    assert!(!zulu.root.exists());
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn workflow_run_and_future_invocation_keep_separate_program_snapshots() {
    let plane = workflow_test_plane(1);
    let mut old_child = stage_workflow_child(&plane);
    let runtime = workflow_runtime(&plane);
    let old_program = snapshot_test_program("old", "old generation");
    let old_catalog = WorkflowCatalog::new([(*old_program).clone()]).expect("old workflow catalog");
    let old_snapshot = Arc::clone(
        old_catalog
            .get(old_program.id())
            .expect("old catalog snapshot"),
    );
    let old_context = workflow_test_context_for_generation(
        &plane,
        1,
        "old generation instructions",
        old_catalog.clone(),
        ApprovalMode::FullAccess,
        None,
    );
    assert_eq!(old_context.resources().revision().get(), 1);
    assert_eq!(
        old_context
            .resources()
            .workflows()
            .get(old_program.id())
            .expect("old workflow resource")
            .description(),
        "old generation"
    );

    let old_policy = old_context.execution_policy();
    let mut future = crate::local_runtime::config::CurrentRuntimeConfig::from_toml_slice(
        b"[agent.model]\nmodel = \"local/model\"\n",
    )
    .unwrap();
    future.model_timeout_policy.response_start_timeout_ms = 41_000;
    future.tool_deadline_policy.hard_deadline_ms = 42_000;
    // Capture precedes publication; the Workflow's child is not prepared
    // until after this native registry publication has completed.
    plane.registry.publish_configuration(&future);
    let new_policy = crate::runtime::subagent::InheritedExecutionPolicy {
        model_timeout: future.timeout_policy().unwrap(),
        tool_deadline: future.tool_deadline_policy().unwrap(),
    };
    assert_ne!(old_policy, new_policy);
    let (_, old_cancellation) = workflow_cancellation();
    let old_task = tokio::spawn({
        let runtime = runtime.clone();
        async move {
            runtime
                .run_test_foreground(
                    old_snapshot,
                    ToolCallId::new("snapshot-old"),
                    old_context.clone(),
                    json!({}),
                    old_cancellation,
                )
                .await
        }
    });
    old_child.expect_delegate().await;
    assert_eq!(
        plane.registry.prepared_execution_policies(),
        vec![old_policy]
    );

    // Construct the replacement immutable generation while the first run
    // is still parked at its native child. The run owns the old Arc and
    // cannot observe this future-invocation program.
    let new_program = snapshot_test_program("new", "new generation");
    let new_catalog = WorkflowCatalog::new([(*new_program).clone()]).expect("new workflow catalog");
    let new_snapshot = Arc::clone(
        new_catalog
            .get(new_program.id())
            .expect("new catalog snapshot"),
    );
    let new_context = workflow_test_context_for_generation(
        &plane,
        2,
        "new generation instructions",
        new_catalog.clone(),
        ApprovalMode::FullAccess,
        None,
    )
    .with_test_policy(new_policy);
    assert_eq!(new_context.resources().revision().get(), 2);
    assert_eq!(
        new_context
            .resources()
            .workflows()
            .get(new_program.id())
            .expect("new workflow resource")
            .description(),
        "new generation"
    );
    old_child
        .send_result(
            crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
            Some(r#"{"summary":"old"}"#),
        )
        .await;
    let old_result = old_task
        .await
        .expect("old workflow task")
        .expect("old workflow success");
    assert_eq!(old_result, json!({"old": {"summary": "old"}}));

    let mut new_child = stage_workflow_child(&plane);
    let (_, new_cancellation) = workflow_cancellation();
    let new_task = tokio::spawn({
        let runtime = runtime.clone();
        async move {
            runtime
                .run_test_foreground(
                    new_snapshot,
                    ToolCallId::new("snapshot-new"),
                    new_context,
                    json!({}),
                    new_cancellation,
                )
                .await
        }
    });
    new_child.expect_delegate().await;
    assert_eq!(
        plane.registry.prepared_execution_policies(),
        vec![old_policy, new_policy]
    );
    new_child
        .send_result(
            crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
            Some(r#"{"summary":"new"}"#),
        )
        .await;
    let new_result = new_task
        .await
        .expect("new workflow task")
        .expect("new workflow success");
    assert_eq!(new_result, json!({"new": {"summary": "new"}}));
    let snapshots = plane.registry.all_snapshots();
    assert_eq!(plane.registry.unsettled_snapshot(), Vec::new());
    assert_eq!(snapshots.len(), 2);
    assert_ne!(
        snapshots[0].definition_digest, snapshots[1].definition_digest,
        "each child retains the named-profile definition from its generation"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workflow_lifecycle_event_append_is_best_effort_observability() {
    let plane = workflow_test_plane(1);
    plane.store.arm_fail_event_times(1);
    let runtime = workflow_runtime(&plane);
    let context = workflow_test_context(&plane);
    let (_, cancellation) = workflow_cancellation();
    let result = runtime
        .run_test_foreground(
            return_only_program(),
            ToolCallId::new("event-best-effort"),
            context,
            json!({"value": "kept"}),
            cancellation,
        )
        .await
        .expect("observability failure does not change workflow authority");
    assert_eq!(result, json!({"value": "kept"}));
    let events = plane
        .store
        .read_events(None, 32)
        .expect("journal read")
        .events;
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.event, RuntimeEvent::WorkflowStarted { .. }))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event.event, RuntimeEvent::WorkflowCompleted { .. }))
    );
}

#[cfg(unix)]
#[tokio::test]
async fn editor_authored_nested_file_executes_keyed_results_and_both_loop_exits() {
    let source = include_str!("../../../tests/fixtures/workflow/visual-authoring.json");
    let document: crate::workflow_file::WorkflowFileDocument =
        serde_json::from_str(source).expect("editor file document");
    document.canonical_bytes().expect("bounded native metadata");
    for broken in [true, false] {
        let plane = workflow_test_plane(1);
        let runtime = workflow_runtime(&plane);
        let native = runtime.read_model.clone();
        let mut definition = document.definition.clone();
        let WorkflowNodeDefinition::Loop {
            break_condition, ..
        } = definition.block.nodes.get_mut("repeat").expect("Loop")
        else {
            panic!("editor fixture owns a Loop");
        };
        *break_condition = Some(WorkflowPredicate::Boolean {
            value: WorkflowValue::Literal {
                value: json!(broken),
            },
        });
        let program = Arc::new(compile_test(definition).expect("editor-authored program"));
        let (_, cancellation) = workflow_cancellation();
        let result = runtime
            .run_test_foreground(
                program,
                ToolCallId::new("editor-nested"),
                workflow_test_context(&plane),
                json!({"message": "verified"}),
                cancellation,
            )
            .await
            .expect("native execution settles");
        assert_eq!(
            result,
            json!({
                "left": {"echo": "verified"}, "right": {"echo": "verified"},
                "loop": {"count": if broken {0} else {3}, "echo": "verified"}
            })
        );
        assert_eq!(
            plane.registry.all_snapshots(),
            [] as [crate::runtime::subagent::SubagentSnapshot; 0]
        );
        let snapshot = native.snapshot();
        assert!(
            snapshot.runs[0]
                .instances
                .iter()
                .all(|node| node.child.is_none())
        );
        assert!(
            !serde_json::to_string(&snapshot)
                .unwrap()
                .contains("Author-only")
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn editor_schema_renames_preserve_native_finite_input_contracts() {
    let cases: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow/schema-rename.json"
    ))
    .unwrap();
    for case in cases.as_array().unwrap() {
        let definition: WorkflowDefinition = serde_json::from_value(json!({"description":"Renamed finite contract","block":{
            "input":case["after"],"nodes": {"workflow_start":{"type":"start"},"done":{"type":"return","outputs":[]}},"edges": [{"from":"workflow_start","to":"done"}],}})).unwrap();
        let program = Arc::new(compile_test(definition).expect("renamed contract compiles"));
        let plane = workflow_test_plane(1);
        let runtime = workflow_runtime(&plane);
        let (_, cancellation) = workflow_cancellation();
        assert_eq!(
            runtime
                .run_test_foreground(
                    program.clone(),
                    ToolCallId::new("renamed-input"),
                    workflow_test_context(&plane),
                    case["input"].clone(),
                    cancellation
                )
                .await
                .unwrap(),
            json!({}),
            "{}",
            case["name"]
        );
        let (_, cancellation) = workflow_cancellation();
        assert!(
            runtime
                .run_test_foreground(
                    program,
                    ToolCallId::new("old-input"),
                    workflow_test_context(&plane),
                    case["oldInput"].clone(),
                    cancellation
                )
                .await
                .is_err(),
            "old key rejected: {}",
            case["name"]
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn editor_rejected_schema_renames_keep_destination_only_finite_inputs_valid() {
    let cases: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/workflow/schema-rename-collisions.json"
    ))
    .unwrap();
    for case in cases.as_array().unwrap() {
        let definition: WorkflowDefinition = serde_json::from_value(json!({"description":"Preserved finite contract","block":{
            "input":case["schema"],"nodes": {"workflow_start":{"type":"start"},"done":{"type":"return","outputs":[]}},"edges": [{"from":"workflow_start","to":"done"}],}})).unwrap();
        let program = Arc::new(compile_test(definition).expect("preserved contract compiles"));
        let plane = workflow_test_plane(1);
        let runtime = workflow_runtime(&plane);
        let (_, cancellation) = workflow_cancellation();
        assert_eq!(
            runtime
                .run_test_foreground(
                    program,
                    ToolCallId::new("preserved-input"),
                    workflow_test_context(&plane),
                    case["input"].clone(),
                    cancellation
                )
                .await
                .unwrap(),
            json!({}),
            "{}",
            case["name"]
        );
    }
}

#[allow(clippy::needless_pass_by_value)]
fn defaulted_definition(tone: Value) -> WorkflowDefinition {
    let mut definition = base_definition(
        "done",
        BTreeMap::from([(
            "done".into(),
            return_node(BTreeMap::from([("tone".into(), reference("args.tone"))])),
        )]),
        Vec::new(),
    );
    definition.block.input = schema(json!({"tone": tone}), &[]);
    definition
}

#[test]
fn optional_input_with_default_is_referenceable_and_bounded() {
    compile_test(defaulted_definition(
        json!({"type": "string", "maxLength": 8, "default": "formal"}),
    ))
    .expect("a defaulted optional field is always present");
    for (tone, rejected) in [
        (
            json!({"type": "string", "maxLength": 8}),
            "unavailable field",
        ),
        (
            json!({"type": "string", "maxLength": 4, "default": "formal"}),
            "default does not satisfy",
        ),
        (
            json!({"type": "string", "maxLength": 0, "default": ""}),
            "maxLength must be",
        ),
        (
            json!({"type": "integer", "maxLength": 4, "default": 1}),
            "only valid for string",
        ),
    ] {
        let error = compile_test(defaulted_definition(tone.clone()))
            .expect_err(&format!("{tone} must be rejected"))
            .to_string();
        assert!(error.contains(rejected), "{tone}: {error}");
    }
}

#[test]
fn schema_defaults_fill_absent_properties_recursively() {
    let schema = json!({"type": "object", "properties": {
        "tone": {"type": "string", "default": "formal"},
        "rows": {"type": "array", "items": {"type": "object", "properties": {"n": {"type": "integer", "default": 1}}}},
        "kept": {"type": "string", "default": "unused"}
    }});
    let mut value = json!({"rows": [{}, {"n": 2}], "kept": "given"});
    apply_schema_defaults(&schema, &mut value).unwrap();
    assert_eq!(
        value,
        json!({"tone": "formal", "rows": [{"n": 1}, {"n": 2}], "kept": "given"})
    );
}

#[test]
fn inserted_default_containers_are_normalized_recursively() {
    let schema = json!({"type":"object","properties": {
        "opts":{"type":"object","properties":{"n":{"type":"integer","default":1}},"additionalProperties":false,"default":{}},
        "rows":{"type":"array","items":{"type":"object","properties":{"n":{"type":"integer","default":2}}},"default":[{}]}
    }});
    for mut input in [json!({}), json!({"opts":{},"rows":[{}]})] {
        apply_schema_defaults(&schema, &mut input).unwrap();
        assert_eq!(input, json!({"opts":{"n":1},"rows":[{"n":2}]}));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_fills_input_defaults_before_validation() {
    let program = Arc::new(
        compile_test(defaulted_definition(
            json!({"type": "string", "maxLength": 8, "default": "formal"}),
        ))
        .expect("program"),
    );
    let plane = workflow_test_plane(1);
    let runtime = workflow_runtime(&plane);
    let (_, cancellation) = workflow_cancellation();
    assert_eq!(
        runtime
            .run_test_foreground(
                program,
                ToolCallId::new("input-default"),
                workflow_test_context(&plane),
                json!({}),
                cancellation,
            )
            .await
            .unwrap(),
        json!({"tone": "formal"})
    );
}

#[cfg(unix)]
fn workflow_runtime(plane: &WorkflowTestPlane) -> WorkflowRuntime {
    WorkflowRuntime::new(
        plane.registry.clone(),
        plane.store.clone(),
        read_model::WorkflowReadModel::new(plane.conversation_id.clone()),
    )
}

#[cfg(unix)]
fn workflow_cancellation() -> (CancellationSignal, ExecutionCancellation) {
    let signal = CancellationSignal::new();
    let cancellation =
        ExecutionCancellation::detached(signal.clone(), CancellationReason::UserRequested);
    (signal, cancellation)
}

#[test]
fn compiles_an_agent_branch_return_dag() {
    let output = schema(
        json!({
            "passed": {"type": "boolean"},
            "summary": {"type": "string"}
        }),
        &["passed", "summary"],
    );
    let definition = WorkflowDefinition {
        timeout_ms: 600_000,
        permissions: None,
        description: "Review".to_owned(),
        block: WorkflowBlock {
            input: schema(json!({"task": {"type": "string"}}), &["task"]),
            nodes: {
                let mut nodes: std::collections::BTreeMap<
                    String,
                    crate::runtime::workflow::WorkflowNodeDefinition,
                > = BTreeMap::from([
                    ("review".to_owned(), agent(output)),
                    (
                        "done".to_owned(),
                        return_node(BTreeMap::from([(
                            "summary".to_owned(),
                            reference("review.summary"),
                        )])),
                    ),
                ]);
                nodes.insert(
                    "workflow_start".into(),
                    crate::runtime::workflow::WorkflowNodeDefinition::Start {},
                );
                nodes
            },
            edges: {
                let mut edges = vec![WorkflowEdgeDefinition {
                    from: "review".to_owned(),
                    to: "done".to_owned(),
                    port: None,
                }];
                edges.insert(
                    0,
                    crate::runtime::workflow::WorkflowEdgeDefinition {
                        from: "workflow_start".into(),
                        to: "review".to_owned(),
                        port: None,
                    },
                );
                edges
            },
        },
    };
    let program = WorkflowProgram::compile(WorkflowId::parse("review_pr").expect("id"), definition)
        .expect("program");
    assert_eq!(program.entry(), "workflow_start");
    assert_eq!(program.nodes().len(), 3);
}

#[test]
fn workflow_output_is_not_a_valid_workflow_identity() {
    assert_eq!(
        WorkflowId::parse(WORKFLOW_OUTPUT_TOOL_NAME),
        Err(WorkflowIdError::ReservedModelName)
    );
}

#[test]
fn rejects_branch_without_boolean_condition_or_complete_ports() {
    let definition = WorkflowDefinition {
        timeout_ms: 600_000,
        permissions: None,
        description: "Branch".to_owned(),
        block: WorkflowBlock {
            input: schema(json!({"flag": {"type": "string"}}), &["flag"]),
            nodes: {
                let mut nodes: std::collections::BTreeMap<
                    String,
                    crate::runtime::workflow::WorkflowNodeDefinition,
                > = BTreeMap::from([(
                    "decision".to_owned(),
                    WorkflowNodeDefinition::Branch {
                        cases: vec![crate::runtime::workflow::WorkflowBranchCase {
                            id: "true".into(),
                            condition: WorkflowPredicate::Boolean {
                                value: reference("args.flag"),
                            },
                        }],
                    },
                )]);
                nodes.insert(
                    "workflow_start".into(),
                    crate::runtime::workflow::WorkflowNodeDefinition::Start {},
                );
                nodes
            },
            edges: {
                let mut edges = Vec::new();
                edges.insert(
                    0,
                    crate::runtime::workflow::WorkflowEdgeDefinition {
                        from: "workflow_start".into(),
                        to: "decision".to_owned(),
                        port: None,
                    },
                );
                edges
            },
        },
    };
    let error = WorkflowProgram::compile(WorkflowId::parse("branch").expect("id"), definition)
        .expect_err("invalid branch");
    assert!(matches!(
        error.cause(),
        WorkflowCompileError::IncompatibleReference(_)
    ));
}

#[test]
fn yaml_is_serialization_and_does_not_supply_identity() {
    let yaml = "description: Review\nblock:\n  input: {type: object, properties: {task: {type: string}}, required: [task]}\n  nodes:\n    workflow_start:\n      type: start\n    done:\n      type: return\n      outputs: [{variable: summary, value_selector: [args, task]}]\n  edges:\n  - from: workflow_start\n    to: done\n";
    let definition: WorkflowDefinition = serde_yaml_ng::from_str(yaml).expect("yaml");
    assert!(WorkflowId::parse("review_pr").is_ok());
    assert!(!yaml.contains("name:"));
    assert!(
        definition
            .block
            .nodes
            .values()
            .any(|node| matches!(node, WorkflowNodeDefinition::Start {}))
    );
}

#[test]
fn duplicate_yaml_node_ids_are_rejected_before_compilation() {
    let yaml = r"
description: Duplicate
block:
  input: {type: object}
  nodes:
    workflow_start:
      type: start
    done: {type: return, outputs: []}
    done: {type: return, outputs: []}
  edges:
  - from: workflow_start
    to: done
";
    assert!(serde_yaml_ng::from_str::<WorkflowDefinition>(yaml).is_err());
}

#[test]
fn rejects_dangling_unreachable_cyclic_and_unterminated_graphs() {
    let dangling = base_definition(
        "done",
        BTreeMap::from([("done".to_owned(), return_node(BTreeMap::new()))]),
        vec![edge("done", "missing")],
    );
    assert!(matches!(
        compile_test(dangling),
        Err(ref error) if matches!(error.cause(), WorkflowCompileError::DanglingReference(_))
    ));

    let unreachable = base_definition(
        "done",
        BTreeMap::from([
            ("done".to_owned(), return_node(BTreeMap::new())),
            ("orphan".to_owned(), return_node(BTreeMap::new())),
        ]),
        Vec::new(),
    );
    assert!(matches!(
        compile_test(unreachable),
        Err(ref error) if matches!(error.cause(), WorkflowCompileError::Unreachable(_))
    ));

    let output = schema(json!({"ok": {"type": "boolean"}}), &["ok"]);
    let cyclic = base_definition(
        "entry",
        BTreeMap::from([
            ("entry".to_owned(), agent(output.clone())),
            ("a".to_owned(), agent(output.clone())),
            ("b".to_owned(), agent(output)),
        ]),
        vec![edge("entry", "a"), edge("a", "b"), edge("b", "a")],
    );
    assert!(matches!(
        compile_test(cyclic),
        Err(ref error) if matches!(error.cause(), WorkflowCompileError::Cycle { .. })
    ));

    let unterminated = base_definition(
        "agent",
        BTreeMap::from([(
            "agent".to_owned(),
            agent(schema(json!({"ok": {"type": "boolean"}}), &["ok"])),
        )]),
        Vec::new(),
    );
    assert!(matches!(
        compile_test(unterminated),
        Err(ref error) if matches!(error.cause(), WorkflowCompileError::Unterminated(_))
    ));
}

#[test]
fn rejects_branch_without_complete_ports() {
    let branch = WorkflowNodeDefinition::Branch {
        cases: vec![crate::runtime::workflow::WorkflowBranchCase {
            id: "true".into(),
            condition: WorkflowPredicate::Boolean {
                value: reference("args.flag"),
            },
        }],
    };
    let missing_ports = WorkflowDefinition {
        timeout_ms: 600_000,
        permissions: None,
        description: "Branch".to_owned(),
        block: WorkflowBlock {
            input: schema(json!({"flag": {"type": "boolean"}}), &["flag"]),
            nodes: {
                let mut nodes: std::collections::BTreeMap<
                    String,
                    crate::runtime::workflow::WorkflowNodeDefinition,
                > = BTreeMap::from([("decision".to_owned(), branch)]);
                nodes.insert(
                    "workflow_start".into(),
                    crate::runtime::workflow::WorkflowNodeDefinition::Start {},
                );
                nodes
            },
            edges: {
                let mut edges = Vec::new();
                edges.insert(
                    0,
                    crate::runtime::workflow::WorkflowEdgeDefinition {
                        from: "workflow_start".into(),
                        to: "decision".to_owned(),
                        port: None,
                    },
                );
                edges
            },
        },
    };
    assert!(matches!(
        compile_test(missing_ports),
        Err(ref error) if matches!(error.cause(), WorkflowCompileError::InvalidBranch(_))
    ));
}

#[test]
fn rejects_unavailable_optional_and_use_before_definition_values() {
    let output = schema(json!({"summary": {"type": "string"}}), &["summary"]);
    let use_before = base_definition(
        "review",
        BTreeMap::from([
            (
                "review".to_owned(),
                WorkflowNodeDefinition::Agent {
                    profile: "reviewer".into(),
                    task: vec![text("Review the input."), reference("later.summary")],
                    output: WorkflowAgentOutput::JsonSchema {
                        schema: output.clone(),
                    },
                    routes: Vec::new(),
                    permissions: None,
                    on_error: WorkflowValueErrorStrategy::default(),
                },
            ),
            ("done".to_owned(), return_node(BTreeMap::new())),
        ]),
        vec![edge("review", "done")],
    );
    assert!(matches!(
        compile_test(use_before),
        Err(ref error) if matches!(error.cause(), WorkflowCompileError::InvalidReference(_))
    ));

    let optional_output = schema(json!({"summary": {"type": "string"}}), &[]);
    let optional = base_definition(
        "review",
        BTreeMap::from([
            ("review".to_owned(), agent(optional_output)),
            (
                "done".to_owned(),
                return_node(BTreeMap::from([(
                    "summary".to_owned(),
                    reference("review.summary"),
                )])),
            ),
        ]),
        vec![edge("review", "done")],
    );
    assert!(matches!(
        compile_test(optional),
        Err(ref error) if matches!(error.cause(), WorkflowCompileError::InvalidReference(_))
    ));
}

#[test]
fn rejects_path_dependent_values_and_return_schema_mismatches() {
    let review_output = schema(json!({"passed": {"type": "boolean"}}), &["passed"]);
    let branch = WorkflowNodeDefinition::Branch {
        cases: vec![crate::runtime::workflow::WorkflowBranchCase {
            id: "true".into(),
            condition: WorkflowPredicate::Boolean {
                value: reference("review.passed"),
            },
        }],
    };
    let path_dependent = base_definition(
        "review",
        BTreeMap::from([
            ("review".to_owned(), agent(review_output)),
            ("decision".to_owned(), branch),
            (
                "yes".to_owned(),
                agent(schema(json!({"summary": {"type": "string"}}), &["summary"])),
            ),
            (
                "no".to_owned(),
                agent(schema(json!({"other": {"type": "string"}}), &["other"])),
            ),
            (
                "join".to_owned(),
                WorkflowNodeDefinition::Agent {
                    profile: "reviewer".into(),
                    task: vec![text("Join the committed facts."), reference("yes.summary")],
                    output: WorkflowAgentOutput::JsonSchema {
                        schema: schema(json!({"ok": {"type": "boolean"}}), &["ok"]),
                    },
                    routes: Vec::new(),
                    permissions: None,
                    on_error: WorkflowValueErrorStrategy::default(),
                },
            ),
            ("done".to_owned(), return_node(BTreeMap::new())),
        ]),
        vec![
            edge("review", "decision"),
            branch_edge("decision", "yes", WorkflowPort::True),
            branch_edge("decision", "no", WorkflowPort::False),
            edge("yes", "join"),
            edge("no", "join"),
            edge("join", "done"),
        ],
    );
    assert!(matches!(
        compile_test(path_dependent),
        Err(ref error) if matches!(error.cause(), WorkflowCompileError::InvalidReference(_))
    ));
}

#[test]
#[allow(clippy::too_many_lines)] // one nested-schema rejection table
fn rejects_nested_schema_mismatches_and_optional_nested_references() {
    let optional_input = WorkflowDefinition {
        description: "Optional nested input".to_owned(),
        timeout_ms: 600_000,
        permissions: None,
        block: WorkflowBlock {
            input: schema(
                json!({
                    "task": {
                        "type": "object",
                        "properties": {"detail": {"type": "string"}},
                        "required": []
                    }
                }),
                &["task"],
            ),
            nodes: {
                let mut nodes: std::collections::BTreeMap<
                    String,
                    crate::runtime::workflow::WorkflowNodeDefinition,
                > = BTreeMap::from([
                    (
                        "review".to_owned(),
                        WorkflowNodeDefinition::Agent {
                            profile: "reviewer".into(),
                            task: vec![text("Review the input."), reference("args.task.detail")],
                            output: WorkflowAgentOutput::JsonSchema {
                                schema: schema(json!({}), &[]),
                            },
                            routes: Vec::new(),
                            permissions: None,
                            on_error: WorkflowValueErrorStrategy::default(),
                        },
                    ),
                    ("done".to_owned(), return_node(BTreeMap::new())),
                ]);
                nodes.insert(
                    "workflow_start".into(),
                    crate::runtime::workflow::WorkflowNodeDefinition::Start {},
                );
                nodes
            },
            edges: {
                let mut edges = vec![edge("review", "done")];
                edges.insert(
                    0,
                    crate::runtime::workflow::WorkflowEdgeDefinition {
                        from: "workflow_start".into(),
                        to: "review".to_owned(),
                        port: None,
                    },
                );
                edges
            },
        },
    };
    assert!(matches!(
        compile_test(optional_input),
        Err(ref error) if matches!(error.cause(), WorkflowCompileError::InvalidReference(_))
    ));
}

fn single_agent_return_definition(agent_output: Value) -> WorkflowDefinition {
    base_definition(
        "review",
        BTreeMap::from([
            ("review".to_owned(), agent(agent_output)),
            (
                "done".to_owned(),
                return_node(BTreeMap::from([(
                    "result".to_owned(),
                    reference("review.result"),
                )])),
            ),
        ]),
        vec![edge("review", "done")],
    )
}

#[test]
fn workflow_schema_compilation_rejects_unsupported_narrowing_and_accepts_safe_subset_values() {
    for (result, keyword) in [
        (json!({"type": "string", "minLength": 5}), "minLength"),
        (json!({"type": "number", "minimum": 1}), "minimum"),
        (
            json!({"type": "array", "items": {"type": "string"}, "minItems": 1}),
            "minItems",
        ),
    ] {
        let narrowed =
            single_agent_return_definition(schema(json!({"result": result}), &["result"]));
        assert!(
            matches!(
                compile_test(narrowed),
                Err(ref error) if matches!(error.cause(), WorkflowCompileError::InvalidSchema(detail) if detail.contains(keyword))
            ),
            "{keyword}"
        );
    }
    let nested = json!({
        "type": "object",
        "properties": {
            "mode": {"type": "string", "const": "ready"},
            "tags": {
                "type": "array",
                "items": {"type": "string", "enum": ["a", "b"]}
            }
        },
        "required": ["mode", "tags"],
        "additionalProperties": false
    });
    let safe = single_agent_return_definition(schema(json!({"result": nested}), &["result"]));
    compile_test(safe).expect("supported object/array/enum/const schemas are compatible");
}

#[test]
fn workflow_catalog_preserves_inert_discovery_and_rejects_duplicates() {
    let definition = base_definition(
        "done",
        BTreeMap::from([("done".to_owned(), return_node(BTreeMap::new()))]),
        Vec::new(),
    );
    let program = compile_test(definition).expect("program");
    let catalog = WorkflowCatalog::new([program.clone()]).unwrap();
    assert!(catalog.enabled_ids().is_empty());
    assert!(catalog.get(program.id()).is_some());
    assert!(matches!(
        WorkflowCatalog::new([program.clone(), program]),
        Err(WorkflowCatalogError::DuplicateDefinition)
    ));
}

#[test]
fn runtime_reference_resolves_args_without_treating_args_as_a_field() {
    let input = json!({"task": "read this"});
    let binding = reference("args.task");
    assert_eq!(
        evaluate_value(&binding, &input, &imbl::OrdMap::new()).expect("reference"),
        json!("read this")
    );
}

#[test]
fn workflow_output_latch_is_exactly_once_and_cancel_is_terminal() {
    let output_schema = schema(json!({"passed": {"type": "boolean"}}), &["passed"]);
    let latch = WorkflowOutputLatch::new(output_schema.clone()).expect("latch");
    assert!(matches!(
        latch.submit(json!({"passed": "not a boolean"})),
        WorkflowOutputSubmission::Invalid(_)
    ));
    assert_eq!(latch.committed_value(), None);
    assert_eq!(
        latch.submit(json!({"passed": true})),
        WorkflowOutputSubmission::Committed
    );
    assert_eq!(
        latch.submit(json!({"passed": false})),
        WorkflowOutputSubmission::Stale
    );
    assert!(!latch.cancel(crate::runtime::types::CancellationReason::UserRequested));
    assert_eq!(latch.committed_value(), Some(json!({"passed": true})));

    let cancelled = WorkflowOutputLatch::new(output_schema).expect("latch");
    assert!(cancelled.cancel(crate::runtime::types::CancellationReason::UserRequested));
    assert_eq!(
        cancelled.submit(json!({"passed": true})),
        WorkflowOutputSubmission::Stale
    );

    let bounded =
        WorkflowOutputLatch::new(schema(json!({"summary": {"type": "string"}}), &["summary"]))
            .expect("latch");
    assert!(matches!(
        bounded.submit(json!({
            "summary": "x".repeat(crate::runtime::subagent::MAX_RESULT_CONTENT_BYTES)
        })),
        WorkflowOutputSubmission::Invalid(message) if message.contains("bounded value size")
    ));
}

#[test]
fn workflow_output_and_cancellation_have_one_linearized_winner() {
    for _ in 0..32 {
        let latch = Arc::new(
            WorkflowOutputLatch::new(schema(json!({"passed": {"type": "boolean"}}), &["passed"]))
                .expect("latch"),
        );
        let barrier = Arc::new(Barrier::new(2));
        let submit_latch = Arc::clone(&latch);
        let submit_barrier = Arc::clone(&barrier);
        let submit = thread::spawn(move || {
            submit_barrier.wait();
            submit_latch.submit(json!({"passed": true}))
        });
        let cancel_latch = Arc::clone(&latch);
        let cancel_barrier = Arc::clone(&barrier);
        let cancel = thread::spawn(move || {
            cancel_barrier.wait();
            cancel_latch.cancel(crate::runtime::types::CancellationReason::UserRequested)
        });
        let submission = submit.join().expect("submission thread");
        let cancelled = cancel.join().expect("cancellation thread");
        let output_won = submission == WorkflowOutputSubmission::Committed;
        assert_ne!(
            output_won, cancelled,
            "exactly one terminal transition wins"
        );
        assert_eq!(latch.committed_value().is_some(), output_won);
    }
}

#[test]
fn workflow_event_ids_are_stable_and_distinct_per_fact() {
    let workflow_id = WorkflowId::parse("review").expect("workflow id");
    let run_id = test_instance("review", "done").block.run;
    let started = RuntimeEvent::WorkflowStarted {
        tool_call_id: Some(ToolCallId::new("run-1")),
        workflow_id: workflow_id.clone(),
        run_id: run_id.clone(),
    };
    let completed = RuntimeEvent::WorkflowCompleted {
        workflow_id,
        run_id,
    };

    assert_eq!(workflow_event_id(&started), workflow_event_id(&started));
    assert_ne!(workflow_event_id(&started), workflow_event_id(&completed));
    assert!(
        workflow_event_id(&started)
            .as_str()
            .starts_with("workflow-event:")
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_snapshot_start_failure_never_alters_execution_outcome() {
    use crate::durable::{JournalStore, ObservationStore};
    use crate::runtime::subagent::ipc::ChildResultStatus;

    let plane = workflow_test_plane(1);
    let mut child = stage_workflow_child(&plane);
    let runtime = workflow_runtime(&plane);
    let context = workflow_test_context(&plane);
    let output = schema(json!({"summary": {"type": "string"}}), &["summary"]);
    let def = base_definition(
        "work",
        BTreeMap::from([
            ("work".into(), agent(output)),
            (
                "done".into(),
                return_node(BTreeMap::from([(
                    "summary".to_owned(),
                    reference("work.summary"),
                )])),
            ),
        ]),
        vec![edge("work", "done")],
    );
    let document = crate::workflow_file::WorkflowFileDocument {
        definition: def,
        editor: None,
    };
    let program = Arc::new(
        WorkflowProgram::compile_document(
            WorkflowId::parse("obs_fault").expect("id"),
            "rev-obs",
            document,
        )
        .expect("compile program"),
    );

    // Inject failure in the atomic start+snapshot transaction.
    plane.store.arm_fail_workflow_snapshot_times(1);

    let (_, cancellation) = workflow_cancellation();
    let task = tokio::spawn({
        let program = Arc::clone(&program);
        async move {
            runtime
                .run_test_foreground(
                    program,
                    ToolCallId::new("obs-fault-call"),
                    context,
                    json!({"task": "produce summary"}),
                    cancellation,
                )
                .await
        }
    });

    child.expect_delegate().await;
    child
        .send_result(
            ChildResultStatus::Succeeded,
            Some(r#"{"summary":"execution succeeded despite start snapshot error"}"#),
        )
        .await;

    let output = task
        .await
        .expect("join workflow task")
        .expect("workflow succeeds despite start snapshot failure");
    assert_eq!(
        output,
        json!({"summary": "execution succeeded despite start snapshot error"})
    );

    // Optional start observability failed cleanly without leaving any WorkflowStarted row or snapshot.
    let events = plane.store.read_events(None, 200).expect("read events");
    assert!(
        !events
            .events
            .iter()
            .any(|e| matches!(e.event, RuntimeEvent::WorkflowStarted { .. }))
    );
    let cut = plane.store.observation_read_cut().expect("cut");
    let missing_run = WorkflowRunId {
        conversation_id: plane.conversation_id.clone(),
        origin: WorkflowRunOrigin::Tool {
            attempt_id: crate::runtime::identity::AttemptId::new("obs-fault-attempt"),
        },
        invocation: 0,
    };
    assert!(
        plane
            .store
            .observation_workflow_run_snapshot_header(&cut, &missing_run)
            .expect("read absent snapshot")
            .is_none()
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // Two overlapping runs gated at child delegation to verify revision isolation.
async fn workflow_concurrent_admission_layout_race_preserves_each_run_snapshot() {
    use crate::durable::{JournalStore, ObservationStore};
    use crate::runtime::subagent::ipc::ChildResultStatus;

    let plane = workflow_test_plane(2);
    let mut child1 = stage_workflow_child(&plane);
    let mut child2 = stage_workflow_child(&plane);
    let runtime = workflow_runtime(&plane);
    let output = schema(json!({"summary": {"type": "string"}}), &["summary"]);
    let def = base_definition(
        "work",
        BTreeMap::from([
            ("work".into(), agent(output)),
            (
                "done".into(),
                return_node(BTreeMap::from([(
                    "summary".to_owned(),
                    reference("work.summary"),
                )])),
            ),
        ]),
        vec![edge("work", "done")],
    );
    let doc1 = crate::workflow_file::WorkflowFileDocument {
        definition: def.clone(),
        editor: Some(
            serde_json::from_value(json!({
                "nodes": [{
                    "address": { "block": [], "node": "work" },
                    "position": { "x": 10.0, "y": 20.0 },
                    "title": "Revision 1"
                }]
            }))
            .expect("editor 1"),
        ),
    };
    let doc2 = crate::workflow_file::WorkflowFileDocument {
        definition: def,
        editor: Some(
            serde_json::from_value(json!({
                "nodes": [{
                    "address": { "block": [], "node": "work" },
                    "position": { "x": 999.0, "y": 888.0 },
                    "title": "Revision 2"
                }]
            }))
            .expect("editor 2"),
        ),
    };
    doc1.validate().expect("doc1 valid");
    let wf_id = WorkflowId::parse("race_wf").expect("wf id");
    let prog1 = Arc::new(
        WorkflowProgram::compile_document(wf_id.clone(), "rev-1", doc1).expect("compile rev1"),
    );
    let prog2 =
        Arc::new(WorkflowProgram::compile_document(wf_id, "rev-2", doc2).expect("compile rev2"));
    assert_eq!(
        prog1.run_snapshot().program_digest,
        prog2.run_snapshot().program_digest
    );
    assert_ne!(
        prog1.run_snapshot().source_revision,
        prog2.run_snapshot().source_revision
    );

    let ctx1 = workflow_test_context(&plane);
    let ctx2 = workflow_test_context(&plane);
    let (_, cancel1) = workflow_cancellation();
    let (_, cancel2) = workflow_cancellation();

    let t1 = tokio::spawn({
        let runtime = runtime.clone();
        async move {
            runtime
                .run_test_foreground(
                    prog1,
                    ToolCallId::new("race-call-1"),
                    ctx1,
                    json!({"task": "run 1"}),
                    cancel1,
                )
                .await
        }
    });
    // Deterministic gate: wait until run 1 reaches active child delegation before starting run 2.
    child1.expect_delegate().await;

    let t2 = tokio::spawn({
        let runtime = runtime.clone();
        async move {
            runtime
                .run_test_foreground(
                    prog2,
                    ToolCallId::new("race-call-2"),
                    ctx2,
                    json!({"task": "run 2"}),
                    cancel2,
                )
                .await
        }
    });
    child2.expect_delegate().await;

    child1
        .send_result(ChildResultStatus::Succeeded, Some(r#"{"summary":"out-1"}"#))
        .await;
    child2
        .send_result(ChildResultStatus::Succeeded, Some(r#"{"summary":"out-2"}"#))
        .await;

    assert!(t1.await.expect("join t1").is_ok());
    assert!(t2.await.expect("join t2").is_ok());

    let events = plane.store.read_events(None, 200).expect("read events");
    let started: Vec<(Option<ToolCallId>, WorkflowRunId)> = events
        .events
        .iter()
        .filter_map(|e| match &e.event {
            RuntimeEvent::WorkflowStarted {
                tool_call_id,
                run_id,
                ..
            } => Some((tool_call_id.clone(), run_id.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(started.len(), 2);

    let cut = plane.store.observation_read_cut().expect("cut");
    for (call_id, run_id) in started {
        let (header, doc) = plane
            .store
            .observation_workflow_run_snapshot_document(&cut, &run_id)
            .expect("read snapshot")
            .expect("snapshot bound");
        let expected = match call_id
            .as_ref()
            .map(super::super::identity::ToolCallId::as_str)
        {
            Some("race-call-1") => ("rev-1", Some("Revision 1")),
            Some("race-call-2") => ("rev-2", Some("Revision 2")),
            other => panic!("unexpected call_id {other:?}"),
        };
        assert_eq!(header.source_revision, expected.0);
        assert_eq!(
            doc.editor
                .as_ref()
                .and_then(|e| e.nodes.first())
                .and_then(|n| n.title.as_deref()),
            expected.1
        );
    }
}
