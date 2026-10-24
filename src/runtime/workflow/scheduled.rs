//! Bounded scheduled-start contracts over native immutable resources.
//!
//! Preparation performs no graph effects and grants no execution authority.
//! The Workflow owner must still retain/revalidate these resources through its
//! configuration/lifecycle fence and durably claim the exact start before use.
pub(crate) mod effects;
pub mod lifecycle;

use super::{
    WorkflowBlockProgram, WorkflowId, WorkflowNodeProgram, WorkflowProgram, WorkflowToolIdentity,
};
use crate::capabilities::ToolSourceId;
use crate::capabilities::selection::ExactToolSelector;
use crate::model::frozen::{FrozenModelInvocation, FrozenSummaryModel};
use crate::model::invocation::ModelBindingRegistry;
use crate::model::session::SessionModelConfig;
use crate::runtime::ApprovalMode;
use crate::runtime::identity::{AgentId, RuntimeResourceRevision};
use crate::runtime::resources::RuntimeResourceSnapshot;
use crate::runtime::subagent::{InheritedExecutionPolicy, ResolvedSubagentSpec};
use crate::scheduler::ScheduleBinding;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::Arc;

/// Changes to compilation or dependency framing require a new contract.
pub const SCHEDULED_WORKFLOW_COMPILER_CONTRACT: u32 = 2;
/// Complete persisted descriptor bound, independent of argument payloads.
pub const MAX_SCHEDULED_WORKFLOW_CONTRACT_BYTES: usize = 64 * 1024;
/// Same initial argument budget as Scheduler reminder text.
pub const MAX_SCHEDULED_WORKFLOW_ARGUMENT_BYTES: usize = 16 * 1024;

/// Derived native graph limits, checked independently of the definition digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduledWorkflowLimits {
    pub steps: u32,
    pub agents: u32,
    pub model_calls: u32,
    pub retained_bytes: u32,
    pub timeout_ms: u64,
}

/// Provider and endpoint semantics, excluding credential source and value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduledProviderIdentity {
    pub provider: String,
    pub endpoint_digest: String,
}

/// Selected source's native transport, policy and credential-reference contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduledSourceIdentity {
    pub source: ToolSourceId,
    pub materialization_digest: String,
}

/// Complete effective dependencies at one static graph location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScheduledWorkflowDependency {
    Tool {
        path: Vec<String>,
        selector: ExactToolSelector,
        definition_digest: String,
        source: Option<ScheduledSourceIdentity>,
    },
    Model {
        path: Vec<String>,
        invocation_digest: String,
        provider: ScheduledProviderIdentity,
    },
    Agent {
        path: Vec<String>,
        definition_digest: String,
        profile_digest: String,
        primary_provider: ScheduledProviderIdentity,
        summary_provider: Option<ScheduledProviderIdentity>,
        sources: Vec<ScheduledSourceIdentity>,
    },
}

/// Inert persistence contract; this value never authorizes a start or Claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinnedScheduledWorkflow {
    pub compiler_contract: u32,
    pub binding: ScheduleBinding,
    pub workspace: PathBuf,
    pub root_agent_id: AgentId,
    pub root_extensions_digest: String,
    pub environment_digest: String,
    pub definition: WorkflowToolIdentity,
    pub arguments_digest: String,
    pub limits: ScheduledWorkflowLimits,
    pub execution_policy: InheritedExecutionPolicy,
    pub approval_mode: ApprovalMode,
    pub dependencies: Vec<ScheduledWorkflowDependency>,
    /// Audit evidence only; unrelated generation changes are not mismatches.
    pub observed_resource_revision: RuntimeResourceRevision,
}

/// A bound Agent node retained off-side, without starting a native child.
pub struct PreparedScheduledAgent {
    pub path: Vec<String>,
    pub spec: ResolvedSubagentSpec,
}

/// Immutable resources retained for the Workflow owner's later fenced Claim.
pub struct PreparedScheduledWorkflow {
    contract: PinnedScheduledWorkflow,
    program: Arc<WorkflowProgram>,
    resources: Arc<RuntimeResourceSnapshot>,
    agents: Vec<PreparedScheduledAgent>,
    models: Vec<(Vec<String>, FrozenModelInvocation)>,
}

impl std::fmt::Debug for PreparedScheduledWorkflow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedScheduledWorkflow")
            .field("contract", &self.contract)
            .finish_non_exhaustive()
    }
}

impl PreparedScheduledWorkflow {
    #[must_use]
    pub const fn contract(&self) -> &PinnedScheduledWorkflow {
        &self.contract
    }
    #[must_use]
    pub const fn program(&self) -> &Arc<WorkflowProgram> {
        &self.program
    }
    #[must_use]
    pub const fn resources(&self) -> &Arc<RuntimeResourceSnapshot> {
        &self.resources
    }
    #[must_use]
    pub fn models(&self) -> &[(Vec<String>, FrozenModelInvocation)] {
        &self.models
    }
    #[must_use]
    pub fn agents(&self) -> &[PreparedScheduledAgent] {
        &self.agents
    }
}

/// Bounded refusal codes. Source contents and credential errors are not projected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduledWorkflowPreparationError {
    BindingMismatch,
    UnpreparedResources,
    RootPermissionDenied,
    DefinitionUnavailable,
    DependencyUnavailable,
    InvalidArguments,
    InvalidPolicy,
    InvalidEndpoint,
    ContractTooLarge,
    UnsupportedCompilerContract,
    ContractChanged,
}

impl std::fmt::Display for ScheduledWorkflowPreparationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "scheduled Workflow preparation refused: {self:?}")
    }
}
impl std::error::Error for ScheduledWorkflowPreparationError {}
type Error = ScheduledWorkflowPreparationError;

/// Native binding and one frozen configuration/resource generation.
/// The caller obtains `binding` from the native Session catalog and retains its
/// ownership fences; accepting a descriptor does not establish that authority.
pub struct ScheduledWorkflowPreparation {
    binding: ScheduleBinding,
    resources: Arc<RuntimeResourceSnapshot>,
    model: SessionModelConfig,
    models: ModelBindingRegistry,
    policy: InheritedExecutionPolicy,
    approval_mode: ApprovalMode,
    root_agent_id: AgentId,
}

impl ScheduledWorkflowPreparation {
    /// Prepare against a configured native generation and Session model intent.
    /// # Errors
    /// Refuses a mismatched conversation, absent configuration or invalid policy.
    pub fn new(
        binding: ScheduleBinding,
        resources: Arc<RuntimeResourceSnapshot>,
        model: SessionModelConfig,
    ) -> Result<Self, Error> {
        if resources.capability().conversation_id() != &binding.conversation_id {
            return Err(Error::BindingMismatch);
        }
        let configuration = resources
            .configuration()
            .ok_or(Error::UnpreparedResources)?;
        let policy = InheritedExecutionPolicy {
            model_timeout: configuration
                .config
                .timeout_policy()
                .map_err(|_| Error::InvalidPolicy)?,
            tool_deadline: configuration
                .config
                .tool_deadline_policy()
                .map_err(|_| Error::InvalidPolicy)?,
            context: configuration.config.context_policy(),
        };
        let models = configuration.models.clone();
        let approval_mode = configuration.config.approval_mode;
        let root_agent_id = configuration.config.agent_id.clone();
        Ok(Self {
            binding,
            resources,
            model,
            models,
            policy,
            approval_mode,
            root_agent_id,
        })
    }

    /// Resolve every native dependency and freeze a bounded inert descriptor.
    /// # Errors
    /// Refuses root permission, unavailable dependencies, invalid input or bounds.
    pub fn prepare(
        &self,
        id: &WorkflowId,
        args: &serde_json::Value,
    ) -> Result<PreparedScheduledWorkflow, Error> {
        if !self
            .resources
            .root_profile()
            .is_some_and(|root| root.workflows.contains(id))
        {
            return Err(Error::RootPermissionDenied);
        }
        if serde_json::to_vec(args)
            .map_err(|_| Error::InvalidArguments)?
            .len()
            > MAX_SCHEDULED_WORKFLOW_ARGUMENT_BYTES
        {
            return Err(Error::InvalidArguments);
        }
        let program = self
            .resources
            .workflows()
            .executable(id)
            .map_err(|_| Error::DefinitionUnavailable)?
            .clone();
        super::execution::validate_commit(program.input_schema(), args)
            .map_err(|_| Error::InvalidArguments)?;
        let mut dependencies = Vec::new();
        let mut agents = Vec::new();
        let mut models = Vec::new();
        self.dependencies(
            &program,
            &program.block,
            &mut dependencies,
            &mut agents,
            &mut models,
        )?;
        let contract = PinnedScheduledWorkflow {
            compiler_contract: SCHEDULED_WORKFLOW_COMPILER_CONTRACT,
            binding: self.binding.clone(),
            workspace: self.resources.capability().workspace_root().to_path_buf(),
            root_agent_id: self.root_agent_id.clone(),
            root_extensions_digest: digest(
                &self
                    .resources
                    .root_profile()
                    .ok_or(Error::RootPermissionDenied)?
                    .extensions
                    .effective_digest_framing(),
            ),
            environment_digest: digest(
                &self
                    .resources
                    .capability()
                    .effective_environment()
                    .authorized_entries(),
            ),
            definition: program.tool_identity(),
            arguments_digest: digest(args),
            limits: ScheduledWorkflowLimits {
                steps: u32::try_from(program.execution_bound)
                    .map_err(|_| Error::ContractTooLarge)?,
                agents: u32::try_from(program.agent_bound).map_err(|_| Error::ContractTooLarge)?,
                model_calls: u32::try_from(program.model_call_bound)
                    .map_err(|_| Error::ContractTooLarge)?,
                retained_bytes: u32::try_from(program.retained_bound)
                    .map_err(|_| Error::ContractTooLarge)?,
                timeout_ms: program.timeout_ms,
            },
            execution_policy: self.policy,
            approval_mode: self.approval_mode,
            dependencies,
            observed_resource_revision: self.resources.revision(),
        };
        bounded(&contract)?;
        Ok(PreparedScheduledWorkflow {
            contract,
            program,
            resources: self.resources.clone(),
            agents,
            models,
        })
    }

    pub(crate) fn with_approval_mode(mut self, approval: ApprovalMode) -> Self {
        self.approval_mode = approval;
        self
    }

    pub(crate) fn execution_context(
        &self,
        capability: crate::capabilities::CapabilityCoordinator,
        start_id: lifecycle::ScheduledWorkflowStartId,
    ) -> crate::runtime::subagent::NativeExecutionContext {
        crate::runtime::subagent::NativeExecutionContext::scheduled(
            capability,
            start_id,
            self.resources.clone(),
            self.model.clone(),
            self.models.clone(),
            self.approval_mode,
            self.policy,
        )
    }

    /// Reconstruct without executing effects or replacing an accepted contract.
    /// # Errors
    /// Refuses malformed/changed contracts and current authorization/dependencies.
    pub fn reconstruct(
        &self,
        saved: &PinnedScheduledWorkflow,
        args: &serde_json::Value,
    ) -> Result<PreparedScheduledWorkflow, Error> {
        bounded(saved)?;
        if saved.compiler_contract != SCHEDULED_WORKFLOW_COMPILER_CONTRACT {
            return Err(Error::UnsupportedCompilerContract);
        }
        let prepared = self.prepare(&saved.definition.workflow_id, args)?;
        let mut expected = saved.clone();
        // Audit revision never substitutes for complete semantic comparison.
        expected.observed_resource_revision = prepared.contract.observed_resource_revision;
        if expected != prepared.contract {
            return Err(Error::ContractChanged);
        }
        Ok(prepared)
    }

    #[allow(clippy::too_many_lines)] // One complete static traversal over native node kinds.
    fn dependencies(
        &self,
        program: &WorkflowProgram,
        block: &WorkflowBlockProgram,
        found: &mut Vec<ScheduledWorkflowDependency>,
        agents: &mut Vec<PreparedScheduledAgent>,
        models: &mut Vec<(Vec<String>, FrozenModelInvocation)>,
    ) -> Result<(), Error> {
        for (node_id, node) in &block.nodes {
            let mut path = block.path.clone();
            path.push(node_id.clone());
            match node {
                WorkflowNodeProgram::Tool { selector, .. } => {
                    let definition = program
                        .frozen_tools
                        .get(selector)
                        .ok_or(Error::DependencyUnavailable)?;
                    let registration = self
                        .resources
                        .capability()
                        .available_tools()
                        .registration(definition)
                        .map_err(|_| Error::DependencyUnavailable)?;
                    if registration.foreground() != crate::tools::deadline::ForegroundPolicy::Leaf {
                        return Err(Error::DependencyUnavailable);
                    }
                    let source = selector
                        .source()
                        .map(|source| {
                            let server = crate::tools::mcp::source_server_id(source);
                            let binding = self
                                .resources
                                .capability()
                                .mcp_servers()
                                .get(&server)
                                .ok_or(Error::DependencyUnavailable)?;
                            source_identity(source, binding)
                        })
                        .transpose()?;
                    found.push(ScheduledWorkflowDependency::Tool {
                        path,
                        selector: selector.clone(),
                        definition_digest: digest(definition),
                        source,
                    });
                }
                WorkflowNodeProgram::Model(model) => {
                    let invocation = self
                        .models
                        .freeze(
                            &model
                                .model
                                .selection()
                                .unwrap_or_else(|| self.model.selection()),
                        )
                        .map_err(|_| Error::DependencyUnavailable)?;
                    let provider = provider_identity(&invocation)?;
                    // The native model semantics contain no credential binding/value.
                    let invocation_digest = digest(&(
                        &invocation.model,
                        &invocation.wire_model,
                        invocation.protocol,
                        invocation.context_window,
                        invocation.model_max_output_tokens,
                        invocation.max_output_tokens,
                        &invocation.profile,
                        invocation.reasoning_enabled,
                        &invocation.request_params,
                        &invocation.capabilities,
                        &invocation.declared_capabilities,
                        &invocation.compat,
                    ));
                    found.push(ScheduledWorkflowDependency::Model {
                        path: path.clone(),
                        invocation_digest,
                        provider,
                    });
                    models.push((path, invocation));
                }
                WorkflowNodeProgram::Agent(agent) => {
                    let frozen = agent
                        .resolved
                        .as_ref()
                        .ok_or(Error::DependencyUnavailable)?;
                    let spec = frozen
                        .bind(&self.resources, &self.model, &self.models)
                        .map_err(|_| Error::DependencyUnavailable)?;
                    if !spec.model.primary.capabilities.tool_calls {
                        return Err(Error::DependencyUnavailable);
                    }
                    let primary_provider = provider_identity(&spec.model.primary)?;
                    let summary_provider = match &spec.model.summary {
                        FrozenSummaryModel::Session => None,
                        FrozenSummaryModel::Explicit(invocation) => {
                            Some(provider_identity(invocation)?)
                        }
                    };
                    let sources = spec
                        .materialization
                        .sources
                        .iter()
                        .map(|(source, binding)| source_identity(source, binding))
                        .collect::<Result<Vec<_>, _>>()?;
                    found.push(ScheduledWorkflowDependency::Agent {
                        path: path.clone(),
                        definition_digest: spec.definition_digest.as_str().to_owned(),
                        // The native profile digest deliberately omits wire IDs.
                        // Scheduled reconstruction must also pin the actual
                        // request models, independently of their catalog keys.
                        profile_digest: digest(&(
                            spec.profile_digest().as_str(),
                            &spec.model.primary.wire_model,
                            match &spec.model.summary {
                                FrozenSummaryModel::Session => None,
                                FrozenSummaryModel::Explicit(invocation) => {
                                    Some(&invocation.wire_model)
                                }
                            },
                        )),
                        primary_provider,
                        summary_provider,
                        sources,
                    });
                    agents.push(PreparedScheduledAgent { path, spec });
                }
                WorkflowNodeProgram::Loop { body, .. } => {
                    self.dependencies(program, body, found, agents, models)?;
                }
                WorkflowNodeProgram::Parallel { branches, .. } => {
                    for branch in branches.values() {
                        self.dependencies(program, &branch.block, found, agents, models)?;
                    }
                }
                WorkflowNodeProgram::Review { .. }
                | WorkflowNodeProgram::Branch { .. }
                | WorkflowNodeProgram::Return { .. } => {}
            }
        }
        Ok(())
    }
}

fn bounded(contract: &PinnedScheduledWorkflow) -> Result<(), Error> {
    if serde_json::to_vec(contract)
        .map_err(|_| Error::ContractTooLarge)?
        .len()
        > MAX_SCHEDULED_WORKFLOW_CONTRACT_BYTES
    {
        return Err(Error::ContractTooLarge);
    }
    Ok(())
}

fn digest(value: &impl Serialize) -> String {
    let value = serde_json::to_value(value).expect("typed Workflow contract serializes");
    let canonical = crate::tools::mcp::identity::canonical_json(&value);
    format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))
}

fn endpoint_identity(endpoint: &str) -> Result<String, Error> {
    let url = url::Url::parse(endpoint).map_err(|_| Error::InvalidEndpoint)?;
    // Embedded authentication cannot enter a public identity's preimage.
    // Native explicit credential fields remain supported and may rotate.
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::InvalidEndpoint);
    }
    Ok(digest(&url.as_str()))
}

fn provider_identity(
    invocation: &FrozenModelInvocation,
) -> Result<ScheduledProviderIdentity, Error> {
    Ok(ScheduledProviderIdentity {
        provider: invocation.binding.provider.to_string(),
        endpoint_digest: endpoint_identity(&invocation.binding.base_url)?,
    })
}

fn source_identity(
    source: &ToolSourceId,
    binding: &crate::tools::mcp::McpServerBinding,
) -> Result<ScheduledSourceIdentity, Error> {
    if let crate::tools::mcp::McpTransportConfig::StreamableHttp { endpoint, .. } =
        &binding.transport
    {
        endpoint_identity(endpoint)?;
    }
    // Native serialization includes transport/policy and credential references;
    // SourceCredentials explicitly skips captured and resolved secret values.
    Ok(ScheduledSourceIdentity {
        source: source.clone(),
        materialization_digest: digest(binding),
    })
}

#[cfg(test)]
mod tests;
