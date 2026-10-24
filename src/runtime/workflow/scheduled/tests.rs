use super::*;
use crate::capabilities::{AvailableToolCatalog, CapabilityAvailability, CapabilitySnapshot};
use crate::local_runtime::configuration::generation::AuthoredConfiguration;
use crate::model::catalog::{MapCredentialEnvironment, ModelCatalog, ModelRef};
use crate::runtime::agent_profile::{
    AgentProfile, AgentProfileAuthority, AgentProfileKind, AgentScope, resolve_agent_profile,
};
use crate::runtime::identity::{
    CapabilityRevision, ConversationId, SessionId, SessionNodeId, ToolId,
};
use crate::runtime::resources::RuntimeConfiguration;
use crate::runtime::subagent::{AgentCatalog, NamedAgentDefinition, SubagentName};
use crate::runtime::workflow::{WorkflowCatalog, WorkflowDefinition};
use crate::skills::SkillSnapshot;
use crate::tools::environment::ToolEnvironment;
use crate::tools::executor::{
    ToolExecutionContext, ToolExecutionHandle, ToolExecutor, ToolRegistration, ToolRegistry,
};
use crate::tools::mcp::McpRuntimeLeaseAuthority;
use crate::tools::types::*;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const EMPTY: &str = r#"{"type":"object","properties":{},"additionalProperties":false}"#;

struct NeverStart;
impl ToolExecutor for NeverStart {
    fn start<'a>(
        &'a self,
        _: ToolInvocation,
        _: ToolExecutionContext<'a>,
    ) -> ToolExecutionHandle<'a> {
        panic!("scheduled preparation must never execute a Tool")
    }
    fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
        crate::tools::deadline::ToolProgressCapability::None
    }
}

#[derive(Clone, Copy)]
enum NodeKind {
    PureReturn,
    Tool,
    Agent,
    Model,
}

struct Inputs<'a> {
    revision: u64,
    description: &'a str,
    instructions: &'a str,
    endpoint: &'a str,
    credential: &'a str,
    wire_model: &'a str,
    summary_wire_model: &'a str,
    permitted: bool,
    kind: NodeKind,
    nested: bool,
    tool_description: &'a str,
    metadata: bool,
}
impl Default for Inputs<'_> {
    fn default() -> Self {
        Self {
            revision: 1,
            description: "Pinned program",
            instructions: "Review the task",
            endpoint: "http://127.0.0.1:9/v1",
            credential: "test-secret-one",
            wire_model: "model",
            summary_wire_model: "summary",
            permitted: true,
            kind: NodeKind::Agent,
            nested: false,
            tool_description: "Check the task",
            metadata: false,
        }
    }
}

fn id() -> WorkflowId {
    WorkflowId::parse("scheduled").unwrap()
}
fn args() -> Value {
    json!({"task":"Review this"})
}

fn program(input: &Inputs<'_>) -> WorkflowProgram {
    if matches!(input.kind, NodeKind::PureReturn) {
        return WorkflowProgram::compile(id(),serde_json::from_value(json!({"description":input.description,"block":{
            "input":{"type":"object","properties":{"task":{"type":"string"}},"required":["task"],"additionalProperties":false},
            "output":{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"],"additionalProperties":false},
            "entry":"done","nodes":{"done":{"type":"return","output":{"type":"literal","value":{"answer":"durable"}}}},"edges":[]
        }})).unwrap()).unwrap();
    }
    let empty: Value = serde_json::from_str(EMPTY).unwrap();
    let work = match input.kind {
        NodeKind::PureReturn => unreachable!("pure definition returned above"),
        NodeKind::Model => {
            json!({"type":"model", "model":{"mode":"session"}, "instruction":"Review the task",
            "input":{}, "output":{"type":"text"}})
        }
        NodeKind::Agent => {
            json!({"type":"agent", "profile":"reviewer", "task":"Review this", "output":empty})
        }
        NodeKind::Tool => {
            json!({"type":"tool", "selector":{"origin":"builtin","name":"check"},
            "arguments":{"type":"literal","value":{}},
            "result":{"type":"json","part":0,"schema":empty}})
        }
    };
    let mut definition: WorkflowDefinition = serde_json::from_value(json!({
        "description":input.description,
        "block":{
            "input":{"type":"object","properties":{"task":{"type":"string"}},"required":["task"],"additionalProperties":false},
            "output":empty,"entry":"work",
            "nodes":{"work":work,"done":{"type":"return","output":{"type":"literal","value":{}}}},
            "edges":[{"from":"work","to":"done"}]
        }
    })).unwrap();
    if input.nested {
        let body = serde_json::to_value(&definition.block).unwrap();
        let repeat = json!({
            "input":body["input"], "output":empty, "entry":"repeat",
            "nodes":{
                "repeat":{"type":"loop","input":{"type":"literal","value":{"task":"Review this"}},
                    "body":body,"until":{"type":"boolean","value":{"type":"literal","value":false}},
                    "carry":{"type":"literal","value":{"task":"Review this"}},"max_iterations":2},
                "done":{"type":"return","output":{"type":"literal","value":{}}}
            },
            "edges":[{"from":"repeat","to":"done","port":"satisfied"},{"from":"repeat","to":"done","port":"exhausted"}]
        });
        let branch =
            json!({"input":{"type":"literal","value":{"task":"Review this"}},"block":repeat});
        definition.block = serde_json::from_value(json!({
            "input":definition.block.input,"output":empty,"entry":"fanout",
            "nodes":{
                "fanout":{"type":"parallel","branches":{"alpha":branch,"beta":branch}},
                "done":{"type":"return","output":{"type":"literal","value":{}}}
            },"edges":[{"from":"fanout","to":"done"}]
        }))
        .unwrap();
    }
    WorkflowProgram::compile(id(), definition).unwrap()
}

#[allow(clippy::too_many_lines)] // Compose an actual immutable native generation, never an Attempt.
fn fixture(root: &std::path::Path, input: &Inputs<'_>) -> ScheduledWorkflowPreparation {
    let configuration_source = "[agent]\nworkflows=['scheduled']\n[agent.model]\nmodel='local/model'\n[agent.tools]\nbuiltin=[]\n";
    let mut config = crate::local_runtime::config::CurrentRuntimeConfig::from_toml_slice(
        configuration_source.as_bytes(),
    )
    .unwrap();
    if !input.permitted {
        config.agent.workflows.clear();
    }
    let model_source = format!(
        r#"
[providers.local]
base_url = "{}"
api_key = "{}"
[models."local/model"]
provider = "local"
id = "model"
protocol = "openai_chat_completions"
context_window = 128000
max_output_tokens = 512
[models."local/model".capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
tool_calls = true
reasoning = false
[models."local/model".compat]
chat_reasoning_replay = "omit"
"#,
        input.endpoint, input.credential
    );
    let mut catalog: toml::Value = toml::from_str(&model_source).unwrap();
    catalog["models"]["local/model"]["id"] = input.wire_model.into();
    let mut summary = catalog["models"]["local/model"].clone();
    summary["id"] = input.summary_wire_model.into();
    catalog["models"]
        .as_table_mut()
        .unwrap()
        .insert("local/summary".into(), summary);
    let model_source = toml::to_string(&catalog).unwrap();
    let models = ModelBindingRegistry::new(
        ModelCatalog::from_toml_slice(model_source.as_bytes())
            .unwrap()
            .resolve(&MapCredentialEnvironment::default())
            .unwrap(),
    )
    .unwrap();
    let mut model = SessionModelConfig::of(ModelRef::parse("local/model").unwrap());
    model.summary_model = crate::model::session::AuxiliaryModelPolicy::Explicit {
        model: ModelRef::parse("local/summary").unwrap(),
        profile: None,
        request_params: crate::model::invocation::RequestParams::new(),
        max_output_tokens: None,
    };
    let reviewer = NamedAgentDefinition::new(
        SubagentName::parse("reviewer").unwrap(),
        AgentProfile {
            description: "Reviewer".into(),
            instructions: input.instructions.into(),
            model: Some(model.clone()),
            execution_deadline: None,
            tools: Vec::new(),
            skills: crate::runtime::agent_profile::AgentSkillSelection::default(),
            project_instructions: crate::runtime::agent_profile::AgentProjectInstructionPolicy {
                inherit: false,
                files: Vec::new(),
            },
            extensions: crate::extensions::NativeAgentExtensionsDocument::default().resolve(),
            agents: BTreeSet::new(),
            workflows: BTreeSet::new(),
        },
        root.join("reviewer.md"),
    )
    .unwrap();
    let agents = AgentCatalog::new([reviewer]).unwrap();
    let empty: Value = serde_json::from_str(EMPTY).unwrap();
    let definition = ToolDefinition {
        id: ToolId::new("tool-check"),
        name: "check".into(),
        description: input.tool_description.into(),
        input_schema: empty,
        execution_policy: ToolExecutionPolicy::ForegroundOnly,
        concurrency_policy: ToolConcurrencyPolicy::Sequential,
        approval_policy: ToolApprovalPolicy::Never,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Builtin,
    };
    let registration = ToolRegistration::plain(definition.clone(), Arc::new(NeverStart));
    let available = if input.metadata {
        AvailableToolCatalog::metadata([definition])
    } else {
        AvailableToolCatalog::new(vec![registration.clone()])
    };
    let registry = ToolRegistry::from_registrations([registration]).unwrap();
    let skills = SkillSnapshot::new(Vec::new());
    let availability = CapabilityAvailability::default();
    let mut catalog = WorkflowCatalog::new([program(input)]).unwrap();
    if input.metadata {
        catalog.admit_metadata(
            &available,
            &availability,
            &skills,
            &agents,
            &BTreeSet::from([ToolId::new("tool-check")]),
        );
    } else {
        catalog.admit(
            &available,
            &availability,
            &skills,
            &agents,
            &BTreeMap::new(),
        );
    }
    let root_profile =
        AgentProfile::from_document(&config.agent, AgentProfileKind::Root, Vec::new()).unwrap();
    let root_profile = resolve_agent_profile(
        &root_profile,
        &AgentProfileAuthority {
            tools: &available,
            availability: &availability,
            skills: &skills,
            agents: &agents.names().into_iter().cloned().collect(),
            workflows: &catalog.enabled_ids(),
            scope: AgentScope::Root,
        },
    );
    let binding = ScheduleBinding {
        session_id: SessionId::new("ses_01900000-0000-7000-8000-000000000001"),
        node_id: SessionNodeId::new("node_36524fd8-f674-7fc2-8125-06d01fee0e18"),
        conversation_id: ConversationId::new("conv_fb777980-3e94-78c5-879a-296623a08f02"),
    };
    let capabilities = CapabilitySnapshot::new(
        binding.conversation_id.clone(),
        root.to_path_buf(),
        CapabilityRevision::new(1),
        Arc::new(registry),
        Arc::new(available),
        Arc::new(skills),
        None,
        None,
        ToolEnvironment::new(),
        Arc::new(McpRuntimeLeaseAuthority::empty()),
        Arc::new(BTreeMap::new()),
    )
    .with_resolved_profile(Some(Arc::new(root_profile)));
    let resources = RuntimeResourceSnapshot::new(
        RuntimeResourceRevision::new(input.revision),
        Vec::new(),
        None,
        crate::context::ContextAssembly::new(),
        Arc::new(capabilities),
    )
    .with_subagent_catalog(agents)
    .with_workflow_catalog(catalog)
    .with_configuration(RuntimeConfiguration {
        config: Arc::new(config),
        models,
        resource_definitions: Vec::new(),
        resource_diagnostics: Vec::new(),
        authored: AuthoredConfiguration {
            component_revisions: BTreeMap::new(),
            source_revisions: BTreeMap::new(),
            effective: crate::local_runtime::configuration::settings::SourceDocumentView::default(),
            provenance: BTreeMap::new(),
        },
    });
    ScheduledWorkflowPreparation::new(binding, Arc::new(resources), model).unwrap()
}

#[test]
fn scheduled_workflow_preparation_and_matching_reconstruction_have_zero_execution_effects() {
    let directory = tempfile::tempdir().unwrap();
    for kind in [NodeKind::Tool, NodeKind::Agent] {
        let current = fixture(
            directory.path(),
            &Inputs {
                kind,
                ..Inputs::default()
            },
        );
        let (prepared, effects) = crate::local_runtime::static_effects::measure(|| {
            current.prepare(&id(), &args()).unwrap()
        });
        assert_no_graph_effects(effects);
        assert_eq!(
            prepared.agents().len(),
            usize::from(matches!(kind, NodeKind::Agent))
        );
        assert_eq!(prepared.contract().dependencies.len(), 1);
        let (reconstructed, effects) = crate::local_runtime::static_effects::measure(|| {
            current.reconstruct(prepared.contract(), &args()).unwrap()
        });
        assert_no_graph_effects(effects);
        assert_eq!(reconstructed.contract(), prepared.contract());
        assert!(Arc::ptr_eq(reconstructed.resources(), &current.resources));
        assert_eq!(reconstructed.program().id(), &id());
    }
}

#[test]
fn scheduled_workflow_reconstruction_allows_unrelated_revision_and_same_provider_key_rotation() {
    let directory = tempfile::tempdir().unwrap();
    let first = fixture(directory.path(), &Inputs::default());
    let saved = first.prepare(&id(), &args()).unwrap();
    let current = fixture(
        directory.path(),
        &Inputs {
            revision: 900,
            credential: "test-secret-rotated",
            ..Inputs::default()
        },
    );
    let (reconstructed, effects) = crate::local_runtime::static_effects::measure(|| {
        current.reconstruct(saved.contract(), &args()).unwrap()
    });
    assert_no_graph_effects(effects);
    assert_eq!(
        reconstructed.contract().observed_resource_revision,
        RuntimeResourceRevision::new(900)
    );
    let encoded = serde_json::to_string(reconstructed.contract()).unwrap();
    for private in [
        "test-secret-one",
        "test-secret-rotated",
        "Review the task",
        "http://127.0.0.1:9/v1",
    ] {
        assert!(!encoded.contains(private));
    }
}

#[test]
fn scheduled_workflow_dependency_changes_refuse_reconstruction_without_effects() {
    let directory = tempfile::tempdir().unwrap();
    let first = fixture(directory.path(), &Inputs::default());
    let saved = first.prepare(&id(), &args()).unwrap();
    for input in [
        Inputs {
            description: "Changed definition",
            ..Inputs::default()
        },
        Inputs {
            instructions: "Changed agent instructions",
            ..Inputs::default()
        },
        Inputs {
            endpoint: "http://127.0.0.1:10/v1",
            ..Inputs::default()
        },
    ] {
        let current = fixture(directory.path(), &input);
        let (result, effects) = crate::local_runtime::static_effects::measure(|| {
            current.reconstruct(saved.contract(), &args())
        });
        assert_no_graph_effects(effects);
        assert_eq!(result.unwrap_err(), Error::ContractChanged);
    }
    let current = fixture(
        directory.path(),
        &Inputs {
            permitted: false,
            ..Inputs::default()
        },
    );
    assert_eq!(
        current.reconstruct(saved.contract(), &args()).unwrap_err(),
        Error::RootPermissionDenied
    );
    assert_eq!(
        first
            .reconstruct(saved.contract(), &json!({"task":"Changed arguments"}))
            .unwrap_err(),
        Error::ContractChanged
    );
}

#[test]
fn scheduled_workflow_direct_tool_contract_changes_and_metadata_only_admission_are_refused() {
    let directory = tempfile::tempdir().unwrap();
    let first = fixture(
        directory.path(),
        &Inputs {
            kind: NodeKind::Tool,
            ..Inputs::default()
        },
    );
    let saved = first.prepare(&id(), &args()).unwrap();
    let changed = fixture(
        directory.path(),
        &Inputs {
            kind: NodeKind::Tool,
            tool_description: "Changed tool contract",
            ..Inputs::default()
        },
    );
    assert_eq!(
        changed.reconstruct(saved.contract(), &args()).unwrap_err(),
        Error::ContractChanged
    );
    let metadata = fixture(
        directory.path(),
        &Inputs {
            kind: NodeKind::Tool,
            metadata: true,
            ..Inputs::default()
        },
    );
    assert_eq!(
        metadata.prepare(&id(), &args()).unwrap_err(),
        Error::DependencyUnavailable
    );
}

#[test]
fn scheduled_workflow_binding_policy_and_persistence_bounds_are_enforced() {
    let directory = tempfile::tempdir().unwrap();
    let current = fixture(directory.path(), &Inputs::default());
    let saved = current.prepare(&id(), &args()).unwrap();
    let mut changed = saved.contract().clone();
    changed.binding.node_id = SessionNodeId::generate();
    assert_eq!(
        current.reconstruct(&changed, &args()).unwrap_err(),
        Error::ContractChanged
    );
    let mut changed = saved.contract().clone();
    changed.limits.steps += 1;
    assert_eq!(
        current.reconstruct(&changed, &args()).unwrap_err(),
        Error::ContractChanged
    );
    changed.compiler_contract += 1;
    assert_eq!(
        current.reconstruct(&changed, &args()).unwrap_err(),
        Error::UnsupportedCompilerContract
    );
    let mut oversized = saved.contract().clone();
    oversized.dependencies = vec![oversized.dependencies[0].clone(); 4000];
    assert_eq!(
        current.reconstruct(&oversized, &args()).unwrap_err(),
        Error::ContractTooLarge
    );
    assert_eq!(
        current
            .prepare(
                &id(),
                &json!({"task":"x".repeat(MAX_SCHEDULED_WORKFLOW_ARGUMENT_BYTES)})
            )
            .unwrap_err(),
        Error::InvalidArguments
    );
    let other = fixture(directory.path(), &Inputs::default());
    let mut wrong_binding = other.binding.clone();
    wrong_binding.conversation_id = ConversationId::generate();
    assert!(matches!(
        ScheduledWorkflowPreparation::new(wrong_binding, other.resources, other.model),
        Err(Error::BindingMismatch)
    ));
}

#[test]
fn scheduled_workflow_public_endpoint_identities_refuse_embedded_authentication() {
    for endpoint in [
        "http://user:password@example.test/v1",
        "https://example.test/v1?api_key=secret",
        "https://example.test/v1#secret",
    ] {
        assert_eq!(
            endpoint_identity(endpoint).unwrap_err(),
            Error::InvalidEndpoint
        );
    }
}

#[test]
fn scheduled_workflow_source_identity_excludes_native_credential_values() {
    let source = ToolSourceId::Mcp(crate::runtime::identity::McpServerId::new("source"));
    let mut first = crate::tools::mcp::McpServerBinding {
        credentials: serde_json::from_value(json!({"environment":{"TOKEN":"$SOURCE_TOKEN"}}))
            .unwrap(),
        transport: crate::tools::mcp::McpTransportConfig::Stdio {
            program: "native-server".into(),
            args: Vec::new(),
            cwd: None,
            environment: BTreeMap::new(),
        },
        policy: ToolInvocationPolicy::default(),
    };
    first
        .credentials
        .capture(crate::credentials::CredentialSnapshot::new([(
            "SOURCE_TOKEN".into(),
            "first-private-value".into(),
        )]));
    let mut rotated = first.clone();
    rotated
        .credentials
        .capture(crate::credentials::CredentialSnapshot::new([(
            "SOURCE_TOKEN".into(),
            "rotated-private-value".into(),
        )]));
    assert_eq!(
        source_identity(&source, &first).unwrap(),
        source_identity(&source, &rotated).unwrap()
    );
    if let crate::tools::mcp::McpTransportConfig::Stdio { program, .. } = &mut rotated.transport {
        *program = "replacement-server".into();
    }
    assert_ne!(
        source_identity(&source, &first).unwrap(),
        source_identity(&source, &rotated).unwrap()
    );
}

/// Native model freezing constructs an adapter; it does not invoke a model.
/// Keep every execution/IO/authority boundary closed during preparation.
fn assert_no_graph_effects(effects: [usize; 11]) {
    let [
        process,
        connect,
        _adapter_construction,
        session,
        state,
        preparation,
        credentials,
        workflow_run,
        interaction,
        resource_publication,
        authority_mutation,
    ] = effects;
    assert_eq!(
        [
            process,
            connect,
            session,
            state,
            preparation,
            credentials,
            workflow_run,
            interaction,
            resource_publication,
            authority_mutation
        ],
        [0; 10]
    );
}
#[test]
fn scheduled_workflow_model_semantics_are_pinned_without_request_effects() {
    let directory = tempfile::tempdir().unwrap();
    let first = fixture(
        directory.path(),
        &Inputs {
            kind: NodeKind::Model,
            ..Inputs::default()
        },
    );
    let (saved, effects) =
        crate::local_runtime::static_effects::measure(|| first.prepare(&id(), &args()).unwrap());
    assert_no_graph_effects(effects);
    assert_eq!(saved.models().len(), 1);
    assert_eq!(saved.contract().limits.model_calls, 1);
    assert!(
        matches!(&saved.contract().dependencies[0], ScheduledWorkflowDependency::Model { path, .. } if path.last().unwrap() == "work")
    );
    let rotated = fixture(
        directory.path(),
        &Inputs {
            kind: NodeKind::Model,
            credential: "rotated-model-secret",
            ..Inputs::default()
        },
    );
    let (prepared, effects) = crate::local_runtime::static_effects::measure(|| {
        rotated.reconstruct(saved.contract(), &args()).unwrap()
    });
    assert_no_graph_effects(effects);
    assert!(
        !serde_json::to_string(prepared.contract())
            .unwrap()
            .contains("rotated-model-secret")
    );
    let changed = fixture(
        directory.path(),
        &Inputs {
            kind: NodeKind::Model,
            endpoint: "http://127.0.0.1:10/v1",
            ..Inputs::default()
        },
    );
    let (result, effects) = crate::local_runtime::static_effects::measure(|| {
        changed.reconstruct(saved.contract(), &args())
    });
    assert_no_graph_effects(effects);
    assert_eq!(result.unwrap_err(), Error::ContractChanged);
    let mut tampered = saved.contract().clone();
    tampered.limits.model_calls = 2;
    assert_eq!(
        first.reconstruct(&tampered, &args()).unwrap_err(),
        Error::ContractChanged
    );
}

#[test]
fn scheduled_workflow_nested_loop_and_parallel_dependencies_keep_distinct_static_paths() {
    let directory = tempfile::tempdir().unwrap();
    for kind in [NodeKind::Tool, NodeKind::Agent, NodeKind::Model] {
        let current = fixture(
            directory.path(),
            &Inputs {
                kind,
                nested: true,
                ..Inputs::default()
            },
        );
        let (saved, effects) = crate::local_runtime::static_effects::measure(|| {
            current.prepare(&id(), &args()).unwrap()
        });
        assert_no_graph_effects(effects);
        assert_eq!(saved.contract().dependencies.len(), 2);
        let paths = saved
            .contract()
            .dependencies
            .iter()
            .map(|dependency| match dependency {
                ScheduledWorkflowDependency::Tool { path, .. }
                | ScheduledWorkflowDependency::Agent { path, .. }
                | ScheduledWorkflowDependency::Model { path, .. } => path,
            })
            .collect::<Vec<_>>();
        assert_ne!(paths[0], paths[1]);
        assert!(paths[0].contains(&"alpha".to_owned()));
        assert!(paths[1].contains(&"beta".to_owned()));
        for path in paths {
            assert!(path.contains(&"repeat".to_owned()));
        }
        let (_, effects) = crate::local_runtime::static_effects::measure(|| {
            current.reconstruct(saved.contract(), &args()).unwrap()
        });
        assert_no_graph_effects(effects);
    }
}

mod durable;

#[test]
fn scheduler85_review_reconstruction_pins_agent_primary_and_summary_wire_models() {
    let directory = tempfile::tempdir().unwrap();
    let saved = fixture(directory.path(), &Inputs::default())
        .prepare(&id(), &args())
        .unwrap();
    for changed in [
        Inputs {
            wire_model: "changed-primary",
            ..Inputs::default()
        },
        Inputs {
            summary_wire_model: "changed-summary",
            ..Inputs::default()
        },
    ] {
        assert_eq!(
            fixture(directory.path(), &changed)
                .reconstruct(saved.contract(), &args())
                .unwrap_err(),
            Error::ContractChanged
        );
    }
    fixture(
        directory.path(),
        &Inputs {
            credential: "rotated",
            ..Inputs::default()
        },
    )
    .reconstruct(saved.contract(), &args())
    .unwrap();
}
