//! Root runtime resource composition: the off-side loader that prepares one
//! atomic resource candidate from the configuration owner's immutable capture,
//! admitted source demand, and catalog admission against that candidate.

use super::super::config::CurrentRuntimeConfig;
use super::super::config::CurrentRuntimeConfigError;
use super::super::configuration::AdmittedSessionConfig;
use crate::capabilities::AgentActivation;
use crate::capabilities::CapabilityCoordinator;
use crate::capabilities::CapabilityResourceInputs;
use crate::model::catalog::CredentialEnvironment;
use crate::model::invocation::ModelBindingRegistry;
use crate::runtime::resources::PreparedRuntimeResources;
use crate::runtime::resources::RuntimeResourceLoadError;
use crate::runtime::subagent::SubagentResolver;
use crate::runtime::workflow::WorkflowCatalog;
use crate::runtime::workflow::WorkflowRuntime;
use crate::skills::SkillDiscoveryConfig;
use crate::tools::executor::ToolRegistry;
use crate::tools::native::NativeToolResources;
use crate::tools::native::register_native_tools;
use futures_util::future::BoxFuture;
use std::collections::BTreeSet;
use std::sync::Arc;

/// Off-side resource composition from the configuration owner’s immutable input capture.
///
/// Owned by the application composition next to the runtime it composed; the
/// configuration owner hands its preparation to
/// [`ConversationRuntime::prepare_configuration`].
pub(crate) struct LocalRuntimeResourceLoader {
    paths: AdmittedSessionConfig,
    native_resources: NativeToolResources,
    workflow_runtime: WorkflowRuntime,
    credentials: Arc<dyn CredentialEnvironment>,
}

impl LocalRuntimeResourceLoader {
    pub(super) fn new(
        paths: AdmittedSessionConfig,
        native_resources: NativeToolResources,
        credentials: Arc<dyn CredentialEnvironment>,
        workflow_runtime: WorkflowRuntime,
    ) -> Self {
        Self {
            paths,
            native_resources,
            workflow_runtime,
            credentials,
        }
    }

    /// Builds one complete candidate off-side from an immutable capture.
    #[allow(clippy::too_many_lines)] // one atomic candidate construction pipeline
    pub(crate) fn prepare<'a>(
        &'a self,
        capability: &'a CapabilityCoordinator,
        capture: crate::local_runtime::configuration::ProspectiveSessionConfig,
        preparation_cancellation: &'a crate::runtime::cancellation::CancellationSignal,
    ) -> BoxFuture<'a, Result<PreparedRuntimeResources, RuntimeResourceLoadError>> {
        Box::pin(async move {
            let config = capture.config.as_ref();
            let models = ModelBindingRegistry::new(
                capture
                    .models
                    .resolve(self.credentials.as_ref())
                    .map_err(|error| RuntimeResourceLoadError::new(error.to_string()))?,
            )
            .map_err(|error| RuntimeResourceLoadError::new(error.to_string()))?;
            let base_environment = config
                .tool_environment()
                .map_err(|error| RuntimeResourceLoadError::new(error.to_string()))?;
            let project_context_files = capture.project_context_files.clone();
            let managed_python = capture.managed_python.clone();
            let subagents = capture.subagents.clone();
            let main_admission = config.agent.agents.iter().cloned().collect::<BTreeSet<_>>();
            let main_catalog = subagents.selected_definitions(&main_admission);
            let mut workflows = capture.workflows.as_ref().clone();
            let mut registry = ToolRegistry::new();
            register_native_tools(
                &mut registry,
                NativeToolResources {
                    subagent_catalog: main_catalog,
                    ..self.native_resources.clone()
                },
                config.native_tools.to_policies(),
            )
            .map_err(|error| {
                RuntimeResourceLoadError::new(format!(
                    "cannot register candidate native tools: {error}"
                ))
            })?;
            crate::tools::native::register_workflow_sources(
                &mut registry,
                &self.workflow_runtime,
                &workflows,
            )
            .map_err(|error| {
                RuntimeResourceLoadError::new(format!(
                    "cannot register candidate Workflow Tools: {error}"
                ))
            })?;
            let skill_discovery = SkillDiscoveryConfig {
                automatic: capture.skill_sources.clone(),
            };
            let mcp_servers = captured_mcp_bindings(config, &self.paths.credentials)
                .map_err(|error| RuntimeResourceLoadError::new(error.to_string()))?;
            let inputs = CapabilityResourceInputs {
                source_demand: admitted_source_demand(
                    config,
                    &subagents,
                    &workflows,
                    managed_python.clone(),
                ),
                base_tool_registry: Arc::new(registry),
                agent_activation: AgentActivation {
                    profile: config.agent.clone(),
                    admitted_agents: subagents.names().into_iter().cloned().collect(),
                    admitted_workflows: workflows.entries().keys().cloned().collect(),
                    project_files: capture.root_agent_project_files.clone(),
                },
                skill_discovery,
                mcp_servers,
                base_environment,
            };
            let prepared = capability
                .prepare_captured_inputs(
                    inputs,
                    capture.skill_discovery.clone(),
                    preparation_cancellation,
                )
                .await;
            let mut candidate = prepared.map_err(|error| {
                RuntimeResourceLoadError::new(format!(
                    "cannot prepare candidate capability resources: {error}"
                ))
            })?;
            validate_workflow_tool_name_collisions(&candidate, &workflows)?;
            candidate
                .admit_workflows(&mut workflows, &subagents, &self.workflow_runtime)
                .map_err(RuntimeResourceLoadError::new)?;
            let prepared = PreparedRuntimeResources::new(
                project_context_files,
                None,
                crate::context::ContextAssembly::new(),
                candidate,
            )
            .with_subagent_catalog(subagents);
            let prepared = prepared
                .with_workflow_catalog(workflows)
                .with_managed_python_catalog(managed_python);
            // The catalog is admitted against the very candidate that is
            // about to be published, and rejection happens entirely
            // off-side: nothing of this candidate generation — catalog,
            // capability state, project instructions, Skills, model
            // selection, or the active generation — has been published yet,
            // so the previous complete generation stays authoritative.
            validate_subagent_catalog(&prepared, &models)?;
            let prepared = prepared.with_configuration(capture.runtime_configuration(models));
            #[cfg(test)]
            super::super::agent_resources::test_support::before_publication(
                capability.current_snapshot().workspace_root(),
            )
            .await;
            Ok(prepared)
        })
    }
    /// Revalidate a successful owned catalog against exactly the adopted
    /// sources, models, instructions and context. No source capture or I/O.
    pub(crate) fn prepare_runtime_catalogs(
        &self,
        mut candidate: crate::capabilities::PreparedCapabilityCandidate,
        adopted: &crate::runtime::resources::RuntimeResourceSnapshot,
    ) -> Result<PreparedRuntimeResources, String> {
        let configuration = adopted
            .configuration()
            .ok_or("missing adopted configuration")?;
        let profile = adopted
            .root_profile()
            .ok_or("missing adopted root profile")?;
        candidate.set_context_profile(
            &configuration.config.agent,
            &profile.project_instructions.files,
        );
        let mut workflows = adopted.workflows().clone();
        validate_workflow_tool_name_collisions(&candidate, &workflows)
            .map_err(|error| error.message)?;
        candidate.admit_workflows(&mut workflows, adopted.subagents(), &self.workflow_runtime)?;
        let prepared = adopted
            .prepare_catalog_update(candidate)
            .with_workflow_catalog(workflows);
        validate_subagent_catalog(&prepared, &configuration.models)
            .map_err(|error| error.message)?;
        Ok(prepared)
    }
}

/// Root sources and explicitly admitted Workflow programs create startup demand.
/// Named Agent profiles are inert until child admission.
pub(in crate::local_runtime) fn admitted_source_demand(
    config: &CurrentRuntimeConfig,
    agents: &crate::runtime::subagent::AgentCatalog,
    workflows: &WorkflowCatalog,
    python: crate::runtime::resources::ManagedPythonCatalog,
) -> crate::capabilities::source::ToolSourceDemand {
    let mut sources = BTreeSet::new();
    {
        let selection = &config.agent.tools;
        sources.extend(
            selection
                .selectors()
                .iter()
                .filter_map(|selector| selector.source().cloned()),
        );
    }
    sources.extend(workflow_source_demand(config, agents, workflows));
    crate::capabilities::source::ToolSourceDemand::new(sources, python)
}

/// Enabled Workflow contracts require complete live definitions before admission.
pub(super) fn workflow_source_demand(
    config: &CurrentRuntimeConfig,
    agents: &crate::runtime::subagent::AgentCatalog,
    workflows: &WorkflowCatalog,
) -> BTreeSet<crate::capabilities::ToolSourceId> {
    let mut sources = BTreeSet::new();
    for (id, entry) in workflows.entries() {
        if !config.agent.workflows.contains(id) {
            continue;
        }
        let program = &entry.source;
        sources.extend(
            program
                .inspect()
                .tools
                .iter()
                .filter_map(|selector| selector.source().cloned()),
        );
        for node in program.agent_nodes() {
            if let Some(profile) = agents.get(&node.profile) {
                sources.extend(
                    profile
                        .tools()
                        .iter()
                        .filter_map(|selector| selector.source().cloned()),
                );
            }
        }
    }
    sources
}

pub(crate) fn captured_mcp_bindings(
    config: &CurrentRuntimeConfig,
    credentials: &crate::credentials::CredentialSnapshot,
) -> Result<crate::tools::mcp::McpServerBindings, CurrentRuntimeConfigError> {
    let mut bindings = config.mcp_bindings()?;
    for binding in bindings.values_mut() {
        binding.credentials.capture(credentials.clone());
    }
    Ok(bindings)
}

/// Admits the prepared catalog against the very capability/Skill/model
/// authority of the candidate generation that will publish it.
pub(super) fn validate_subagent_catalog(
    prepared: &PreparedRuntimeResources,
    models: &ModelBindingRegistry,
) -> Result<(), RuntimeResourceLoadError> {
    let candidate = prepared.capability_candidate();
    let skills = crate::skills::SkillSnapshot::new(candidate.skill_packages().to_vec());
    let selected = candidate
        .resolved_profile()
        .map(|profile| profile.agents.clone())
        .unwrap_or_default();
    let admitted = prepared.subagent_catalog().selected_definitions(&selected);
    SubagentResolver::validate_catalog(
        &admitted,
        candidate.available_tools(),
        candidate.availability(),
        &skills,
        models,
    )
    .map_err(|(agent, error)| RuntimeResourceLoadError::new(format!("agents.{agent}: {error}")))
}

/// Rejects a model-facing Workflow id that is already used by another
/// capability in the same candidate generation. The active Tool selection can
/// hide a duplicate under `noDirectTools`, but hiding it must not turn an identity
/// collision into a valid configuration.
pub(super) fn validate_workflow_tool_name_collisions(
    candidate: &crate::capabilities::PreparedCapabilityCandidate,
    workflows: &WorkflowCatalog,
) -> Result<(), RuntimeResourceLoadError> {
    for workflow_id in workflows.entries().keys() {
        let expected_id = format!("tool-workflow-{workflow_id}");
        if workflow_id.as_str() == crate::tools::native::SUBAGENT_TOOL_NAME {
            return Err(RuntimeResourceLoadError::new(format!(
                "Workflow id {workflow_id:?} is reserved by the native subagent Tool"
            )));
        }
        if let Some(conflict) = candidate.available_tools().tools().iter().find(|tool| {
            tool.definition.name == workflow_id.as_str()
                && tool.definition.id.as_str() != expected_id
        }) {
            return Err(RuntimeResourceLoadError::new(format!(
                "Workflow id {workflow_id:?} collides with Tool {} ({})",
                conflict.definition.name, conflict.definition.id
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::capabilities::{ToolSourceId, selection::AgentToolSelection};
    use crate::runtime::subagent::{AgentCatalog, NamedAgentDefinition, SubagentName};
    use crate::runtime::workflow::WorkflowCatalog;

    #[test]
    fn exact_workflow_leaves_and_agent_profile_sources_coalesce_source_demand() {
        use crate::runtime::workflow::{WorkflowId, WorkflowProgram};
        use serde_json::json;
        let mut config: CurrentRuntimeConfig = serde_json::from_value(json!({"agent": {"model": {"model":"local/test"}, "tools": {"sources":{"github":"all"}}}}))
        .unwrap();
        let id = WorkflowId::parse("source-demand").unwrap();
        config.agent.workflows = vec![id.clone()];
        let selector = json!({"origin":"source","source_id":"github","name":"get_issue"});
        let authored = json!({
            "description":"Demand contract",
            "block":{
                "input":{"type":"object"}, "output":{"type":"object"},"nodes": {"workflow_start":{"type":"start"},
                    "leaf":{"type":"tool","selector":selector,"arguments":{"type":"literal","value":{}},
                        "result":{"type":"json","schema":{"type":"object"}}},
                    "agent":{"type":"agent","profile":"reviewer","task":[{"type":"literal","value":"Review"}],"output":{"type":"json_schema","schema":{"type":"object"}}},
                    "done":{"type":"return","output":{"type":"reference","path":["agent"]}}
                },"edges": [{"from":"workflow_start","to":"leaf"},{"from":"leaf","to":"agent"},{"from":"agent","to":"done"}],}
        });
        let workflows = WorkflowCatalog::new([WorkflowProgram::compile(
            id.clone(),
            serde_json::from_value(authored).unwrap(),
        )
        .unwrap()])
        .unwrap();
        let agents = |text: &str| {
            let document = super::super::super::agent_resources::parse(text).unwrap();
            AgentCatalog::new([NamedAgentDefinition::new(
                SubagentName::parse("reviewer").unwrap(),
                crate::runtime::agent_profile::AgentProfile::from_document(
                    &document,
                    crate::runtime::agent_profile::AgentProfileKind::Named,
                    Vec::new(),
                )
                .unwrap(),
                "reviewer.toml".into(),
            )
            .unwrap()])
            .unwrap()
        };
        let demand = |config: &CurrentRuntimeConfig, agents: &AgentCatalog| {
            admitted_source_demand(
                config,
                agents,
                &workflows,
                crate::runtime::resources::ManagedPythonCatalog::default(),
            )
            .sources
        };
        let sourced = agents(
            "description='Review'\ninstructions='Review'\n[tools.sources]\ngithub='all'\n'python:data-analysis'='all'",
        );
        let expected = [
            ToolSourceId::Mcp(crate::runtime::identity::McpServerId::new("github")),
            ToolSourceId::ManagedPython("data-analysis".into()),
        ]
        .into();
        assert_eq!(demand(&config, &sourced), expected);
        config.agent.tools = crate::capabilities::selection::ToolSelectionDocument::default();
        assert_eq!(demand(&config, &sourced), expected);
        // Without profile sources, the exact leaf alone still demands github.
        assert_eq!(
            demand(
                &config,
                &agents("description='Review'\ninstructions='Review'")
            ),
            [ToolSourceId::Mcp(
                crate::runtime::identity::McpServerId::new("github")
            )]
            .into()
        );
    }

    #[test]
    fn allowlisting_named_agents_does_not_materialize_their_sources() {
        let shared = ToolSourceId::ManagedPython("shared".into());
        let unused = ToolSourceId::ManagedPython("unused".into());
        let make = |name: &str, source: ToolSourceId| {
            NamedAgentDefinition::new(
                SubagentName::parse(name).unwrap(),
                crate::runtime::agent_profile::AgentProfile {
                    description: "Test Agent".into(),
                    instructions: "Test instructions".into(),
                    model: None,
                    execution_deadline: None,
                    tools: vec![AgentToolSelection::All { source_id: source }],
                    skills: crate::runtime::agent_profile::AgentSkillSelection::default(),
                    project_instructions:
                        crate::runtime::agent_profile::AgentProjectInstructionPolicy {
                            inherit: false,
                            files: Vec::new(),
                        },
                    extensions: crate::extensions::NativeAgentExtensions::none(),
                    agents: std::collections::BTreeSet::default(),
                    workflows: std::collections::BTreeSet::default(),
                },
                PathBuf::from("instructions.md"),
            )
            .unwrap()
        };
        let agents = AgentCatalog::new([
            make("alpha", shared.clone()),
            make("beta", shared.clone()),
            make("unused", unused),
        ])
        .unwrap();
        let mut config: CurrentRuntimeConfig = serde_json::from_value(
            serde_json::json!({"agent": {"model": {"model": "local/test"}}}),
        )
        .unwrap();
        let empty = || crate::runtime::resources::ManagedPythonCatalog::default();
        assert!(
            admitted_source_demand(&config, &agents, &WorkflowCatalog::empty(), empty())
                .sources
                .is_empty()
        );
        config.agent.agents = ["beta", "alpha"]
            .map(|name| SubagentName::parse(name).unwrap())
            .into();
        let first = admitted_source_demand(&config, &agents, &WorkflowCatalog::empty(), empty());
        config.agent.agents.reverse();
        let second = admitted_source_demand(&config, &agents, &WorkflowCatalog::empty(), empty());
        assert!(first.sources.is_empty());
        assert_eq!(first.sources, second.sources);
        config.agent.tools.sources.insert(
            shared.clone(),
            crate::capabilities::selection::SourceToolSelection::All,
        );
        assert_eq!(
            admitted_source_demand(&config, &agents, &WorkflowCatalog::empty(), empty()).sources,
            [shared].into()
        );
    }
}
