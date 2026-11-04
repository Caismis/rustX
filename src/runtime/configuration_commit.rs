//! Native automatic configuration commit.
//!
//! Configuration authoring (source capture, merge, provenance, redaction and
//! unit composition) belongs to the application configuration owner. The
//! runtime owns what happens after that: an off-side prepared candidate staged
//! for this runtime, the commit at the runtime's own safe boundary, the frozen
//! configuration facts of an admitted Attempt, and the typed record of what a
//! commit actually changed.
//!
//! A staged candidate is never visible to an admitted Attempt. The runtime
//! commits it under the coordinator lock, before it admits the next
//! independent Attempt, once no real execution owner blocks replacement. No
//! client participates; cache impact is never a reason to wait.
//!
//! None of these values interprets authored documents; a candidate carries
//! the application-composed [`RuntimeConfiguration`](crate::runtime::resources::RuntimeConfiguration)
//! inside its preview generation unchanged.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::model::session::SessionModelState;
use crate::runtime::resources::RuntimeResourceSnapshot;

/// Compose a live catalog observation over one exact committed resource binding.
/// This synchronous seam cannot read authored sources or start discovery.
pub(crate) type RuntimeCatalogPreparer = dyn Fn(
        crate::capabilities::PreparedCapabilityCandidate,
        &RuntimeResourceSnapshot,
    ) -> Result<crate::runtime::resources::PreparedRuntimeResources, String>
    + Send
    + Sync;

// The configuration facts an admitted Attempt froze at its admission
// linearization point; a later commit never mutates them. (A plain comment:
// this wire type's schema carries no description.)
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct AdmittedConfiguration {
    pub approval_mode: crate::runtime::ApprovalMode,
    pub model_timeout: Option<crate::local_runtime::config::ModelTimeoutPolicyDocument>,
    pub tool_deadline: Option<crate::local_runtime::config::ToolDeadlinePolicyDocument>,
    pub attempt: crate::runtime::identity::AttemptId,
    pub generation: crate::runtime::identity::RuntimeResourceRevision,
    pub model: crate::model::session::SessionModelView,
    pub resources: crate::runtime::capability_inspection::CapabilityInspection,
}

/// One off-side configuration candidate prepared by a specific runtime
/// allocation against one exact Session binding revision.
pub(crate) struct PreparedConfiguration {
    /// The runtime allocation that prepared this candidate. A candidate from
    /// a replaced runtime can never commit into its successor.
    pub(crate) owner: Arc<()>,
    /// The Session binding revision this candidate was prepared against.
    pub(crate) baseline: u64,
    /// The complete candidate generation used for comparison and, without a
    /// capability candidate, for the commit itself.
    pub(crate) preview: Arc<RuntimeResourceSnapshot>,
    pub(crate) model: SessionModelState,
    pub(crate) capability: Option<crate::capabilities::PreparedCapabilityCandidate>,
    pub(crate) resources: Option<crate::runtime::resources::PreparedRuntimeResourceData>,
}

/// The application owner's latest desired application for one scope.
///
/// The owner advances the shared value under its source fence whenever newer
/// input is desired; the runtime reads it under its coordinator lock at the
/// commit point. A candidate whose own desire is no longer current is
/// superseded and can never commit, even when it finished preparing later.
#[derive(Clone, Debug)]
pub(crate) struct DesiredApplication {
    current: Arc<AtomicU64>,
    attempt: u64,
}

impl DesiredApplication {
    pub(crate) fn new(current: Arc<AtomicU64>, attempt: u64) -> Self {
        Self { current, attempt }
    }

    pub(crate) fn is_current(&self) -> bool {
        self.current.load(Ordering::SeqCst) == self.attempt
    }
}

/// What happened to one staged candidate. Delivered exactly once, outside the
/// coordinator lock, to the application owner that staged it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StagedOutcome {
    /// The candidate is the Session's committed configuration.
    Committed,
    /// Newer desired input, a newer Session binding or a replaced runtime
    /// superseded the candidate before it could commit. Nothing changed.
    Superseded,
    /// The commit was refused; the previous configuration remains in use.
    Failed { diagnostic: String },
    /// The resources were published but their change record could not be
    /// made durable. Admission is fenced until native recovery reconciles.
    Uncertain { diagnostic: String },
}

/// One fully prepared candidate waiting for the runtime's safe boundary.
pub(crate) struct StagedConfiguration {
    pub(crate) prepared: PreparedConfiguration,
    pub(crate) desired: DesiredApplication,
    /// Why this commit happens; recorded with its change fact.
    pub(crate) reason: ConfigurationChangeReason,
    /// Installs the retained Session binding at the committed revision. It
    /// runs inside the commit critical section and takes only leaf locks.
    pub(crate) retain: Box<dyn FnOnce(u64) + Send>,
    /// Reports the outcome to the staging owner; the runtime spawns the future
    /// after releasing its lock, so the owner may take its own fence.
    pub(crate) settled:
        Box<dyn FnOnce(StagedOutcome) -> futures_util::future::BoxFuture<'static, ()> + Send>,
}

/// Why the Session's configuration changed.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ConfigurationChangeReason {
    /// A saved or externally edited authored source.
    Source,
    /// An explicit Session model selection reconsidered the source.
    ModelSelection,
    /// The runtime allocation was rebuilt and reprepared its sources.
    Reallocation,
}

/// One configuration domain a commit changed.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ConfigurationDomain {
    /// The effective system instructions, project guidance or Agent profile.
    Instructions,
    /// The context budgeting/projection policy.
    ContextPolicy,
    /// The model-facing Tool catalog.
    Tools,
    /// The Skill catalog.
    Skills,
    /// The effective model of future work.
    Model,
    /// The execution policy future Attempts freeze: approval mode, model
    /// timeout, Tool deadline and MCP startup policies.
    ExecutionPolicy,
}

/// Most names one change category records; the total stays exact.
pub const CONFIGURATION_CHANGE_NAME_LIMIT: usize = 16;

/// Most JSON-encoded bytes the recorded names of one category occupy. Names
/// are not bounded at their source (an MCP server chooses its own), so the
/// six categories of one change stay within the 16 KiB transcript and Trace
/// headers by this budget, never by an assumed name length.
pub const CONFIGURATION_CHANGE_NAME_BYTES: usize = 1024;

/// A bounded, ordered list of names with its exact total.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct BoundedNames {
    /// Sorted names: at most [`CONFIGURATION_CHANGE_NAME_LIMIT`] of them,
    /// within [`CONFIGURATION_CHANGE_NAME_BYTES`] encoded bytes.
    pub names: Vec<String>,
    /// The exact number of names in this category.
    pub total: u32,
}

impl BoundedNames {
    fn from_names(mut names: Vec<String>) -> Self {
        names.sort();
        let total = u32::try_from(names.len()).unwrap_or(u32::MAX);
        // A name that alone would overflow the budget is counted, not
        // recorded; the shorter names after it still are.
        let mut bytes = 0;
        names.retain(|name| {
            let encoded = serde_json::to_string(name).map_or(usize::MAX, |encoded| encoded.len());
            let fits = bytes + encoded < CONFIGURATION_CHANGE_NAME_BYTES;
            if fits {
                bytes += encoded + 1;
            }
            fits
        });
        names.truncate(CONFIGURATION_CHANGE_NAME_LIMIT);
        Self { names, total }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.total == 0
    }
}

/// Named-item differences of one catalog. A same-name item whose definition
/// changed is `changed`, never a removal plus an addition.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct NamedChanges {
    pub added: BoundedNames,
    pub removed: BoundedNames,
    pub changed: BoundedNames,
}

impl NamedChanges {
    pub(crate) fn between<T: PartialEq>(
        before: &BTreeMap<String, T>,
        after: &BTreeMap<String, T>,
    ) -> Self {
        let added = after
            .keys()
            .filter(|name| !before.contains_key(*name))
            .cloned()
            .collect();
        let removed = before
            .keys()
            .filter(|name| !after.contains_key(*name))
            .cloned()
            .collect();
        let changed = after
            .iter()
            .filter(|(name, value)| before.get(*name).is_some_and(|old| old != *value))
            .map(|(name, _)| name.clone())
            .collect();
        Self {
            added: BoundedNames::from_names(added),
            removed: BoundedNames::from_names(removed),
            changed: BoundedNames::from_names(changed),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

/// The typed semantic summary of one actual configuration commit.
///
/// Recorded once per commit that changed something; never for preparation,
/// a file notification or a commit with no effective semantic change. It
/// names what changed, not the bodies: the exact prompt and Tool schemas a
/// request used remain in that request's own immutable snapshot.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct ConfigurationChange {
    pub reason: ConfigurationChangeReason,
    /// The Session binding revision this commit replaced.
    pub previous_binding_revision: u64,
    /// The Session binding revision this commit installed.
    pub binding_revision: u64,
    /// The changed domains, in their stable order.
    pub domains: Vec<ConfigurationDomain>,
    pub tools: NamedChanges,
    pub skills: NamedChanges,
    /// The byte length of the effective system instructions this commit
    /// installed. The fact retains them exactly, readable in bounded ranges
    /// even when no request ever used them.
    pub instructions_bytes: u64,
    /// The byte length of the retained JSON catalog of every Tool and Skill
    /// this commit added, changed or removed, with their before and after
    /// states.
    pub catalog_bytes: u64,
}

/// The exact content one configuration change fact retains beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfigurationContent {
    /// The effective system instructions.
    pub(crate) instructions: String,
    /// The JSON catalog of the affected Tools and Skills, each with its
    /// `before` and `after` state: a Tool's definition with its implementation
    /// identity (a digest of the server's transport and policy, never its
    /// credentials, the enabled Workflow program and the Named Agents it
    /// runs), a Skill's entry with its content version.
    pub(crate) catalog: String,
}

impl ConfigurationChange {
    /// The effective differences between two committed generations with the
    /// successor's effective instructions, or `None` when nothing future work
    /// observes changed. An independently published unit changes the
    /// generation without rebinding the Session, so both revisions are then
    /// the same.
    pub(crate) fn between(
        reason: ConfigurationChangeReason,
        (previous_binding_revision, binding_revision): (u64, u64),
        before: (&RuntimeResourceSnapshot, &SessionModelState),
        after: (&RuntimeResourceSnapshot, &SessionModelState),
    ) -> Option<(Self, ConfigurationContent)> {
        let (old, old_model) = before;
        let (new, new_model) = after;
        let prompt = |snapshot: &RuntimeResourceSnapshot| {
            snapshot
                .context_assembly()
                .system_sections(&snapshot.native_system_input())
                .map(|sections| crate::context::render_effective_system_prompt(&sections))
                .ok()
        };
        let configuration = |snapshot: &RuntimeResourceSnapshot| {
            snapshot.configuration().map(|configuration| {
                let config = &configuration.config;
                (
                    config.context,
                    (
                        config.approval_mode,
                        config.model_timeout_policy,
                        config.tool_deadline_policy,
                        config.mcp_startup_policy,
                    ),
                )
            })
        };
        let (old_tools, new_tools) = (tool_entries(old), tool_entries(new));
        let (old_skills, new_skills) = (skill_entries(old), skill_entries(new));
        let catalog = serde_json::json!({
            "tools": affected(&old_tools, &new_tools),
            "skills": affected(&old_skills, &new_skills),
        })
        .to_string();
        let tools = NamedChanges::between(&old_tools, &new_tools);
        let skills = NamedChanges::between(&old_skills, &new_skills);
        let mut domains = Vec::new();
        let instructions = prompt(new);
        if prompt(old) != instructions {
            domains.push(ConfigurationDomain::Instructions);
        }
        let (old_configuration, new_configuration) = (configuration(old), configuration(new));
        if old_configuration.map(|c| c.0) != new_configuration.map(|c| c.0) {
            domains.push(ConfigurationDomain::ContextPolicy);
        }
        if !tools.is_empty() {
            domains.push(ConfigurationDomain::Tools);
        }
        if !skills.is_empty() {
            domains.push(ConfigurationDomain::Skills);
        }
        if old_model.view() != new_model.view() || !same_invocations(before, after) {
            domains.push(ConfigurationDomain::Model);
        }
        // Every Tool runs in the authorized environment, so its values are
        // execution policy; the fact never carries them.
        if old_configuration.map(|c| c.1) != new_configuration.map(|c| c.1)
            || Environment::of(old) != Environment::of(new)
        {
            domains.push(ConfigurationDomain::ExecutionPolicy);
        }
        let instructions = instructions.unwrap_or_default();
        (!domains.is_empty()).then_some((
            Self {
                reason,
                previous_binding_revision,
                binding_revision,
                domains,
                tools,
                skills,
                instructions_bytes: instructions.len() as u64,
                catalog_bytes: catalog.len() as u64,
            },
            ConfigurationContent {
                instructions,
                catalog,
            },
        ))
    }
}

/// One Tool as a commit compares it and its catalog retains it: the model
/// declaration, the whole definition and its implementation identity.
#[derive(Debug, PartialEq, serde::Serialize)]
struct ToolEntry {
    definition: crate::tools::types::ToolDefinition,
    model_input_schema: serde_json::Value,
    /// Compared whole, retained only as `server_binding_digest`.
    #[serde(skip)]
    binding: Option<crate::tools::mcp::McpServerBinding>,
    server_binding_digest: Option<String>,
    /// The enabled Workflow program it runs.
    workflow: Option<crate::runtime::workflow::WorkflowToolIdentity>,
    /// The leaf Tools that Workflow's direct Tool nodes call.
    runs: Dependencies,
    /// The Named Agents it runs: those a Workflow froze at admission, or
    /// those the `subagent` intrinsic admits.
    agents: BTreeMap<String, AgentEntry>,
}

/// The Tools something runs beneath a Tool, as the generation resolves them:
/// each leaf definition by Tool id, and the binding of every Tool source it
/// selects, prepared yet or not, so a source still waiting to be prepared
/// hides nothing else.
#[derive(Debug, Default, PartialEq, serde::Serialize)]
struct Dependencies {
    tools: BTreeMap<String, crate::tools::types::ToolDefinition>,
    sources: BTreeMap<String, SourceEntry>,
}

/// One Tool source as a commit compares it: its whole captured binding,
/// credentials included, as a Root Tool's own binding compares. The catalog
/// retains only its transport-and-policy digest (`null` without a binding).
#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(transparent)]
struct SourceEntry {
    #[serde(skip)]
    binding: Option<crate::tools::mcp::McpServerBinding>,
    binding_digest: Option<String>,
}

impl Dependencies {
    fn new<'a>(
        snapshot: &RuntimeResourceSnapshot,
        tools: impl IntoIterator<Item = &'a crate::tools::types::ToolDefinition>,
        sources: impl IntoIterator<Item = crate::capabilities::ToolSourceId>,
    ) -> Self {
        use crate::capabilities::ToolSourceId;
        use crate::tools::types::ToolOrigin;
        let mut selected = sources
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>();
        let tools = tools
            .into_iter()
            .map(|tool| {
                match &tool.origin {
                    ToolOrigin::Builtin => {}
                    ToolOrigin::Mcp { server_id } => {
                        selected.insert(ToolSourceId::Mcp(server_id.clone()));
                    }
                    ToolOrigin::ManagedPython { package } => {
                        selected.insert(ToolSourceId::ManagedPython(package.clone()));
                    }
                }
                (tool.id.to_string(), tool.clone())
            })
            .collect();
        let servers = snapshot.capability().mcp_servers();
        let sources = selected
            .into_iter()
            .map(|source| {
                let server = match &source {
                    ToolSourceId::Mcp(id) => id.clone(),
                    ToolSourceId::ManagedPython(package) => {
                        crate::tools::python::python_server_id(package)
                    }
                };
                let binding = servers.get(&server).cloned();
                (
                    source.to_string(),
                    SourceEntry {
                        binding_digest: binding.as_ref().map(binding_digest),
                        binding,
                    },
                )
            })
            .collect();
        Self { tools, sources }
    }
}

/// One Named Agent as a Tool runs it: its definition and what that
/// definition resolves to in the generation — its Tools and sources, the
/// content version of each Skill it selects, and its project instructions.
/// Its explicit model is compared with the Model domain.
#[derive(Debug, PartialEq, serde::Serialize)]
struct AgentEntry {
    definition: crate::runtime::subagent::NamedAgentDefinitionDigest,
    runs: Dependencies,
    skills: BTreeMap<String, Option<crate::runtime::identity::SkillVersionId>>,
    project_instructions: String,
    environment: Environment,
}

/// The authorized Tool environment a child inherits, compared by its exact
/// entries. The catalog retains only the variable names, never a value.
#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(transparent)]
struct Environment {
    #[serde(skip)]
    entries: Vec<(String, String)>,
    names: Vec<String>,
}

impl Environment {
    fn of(snapshot: &RuntimeResourceSnapshot) -> Self {
        let entries = snapshot
            .capability()
            .effective_environment()
            .authorized_entries()
            .to_vec();
        Self {
            names: entries.iter().map(|(name, _)| name.clone()).collect(),
            entries,
        }
    }
}

impl AgentEntry {
    fn new(
        snapshot: &RuntimeResourceSnapshot,
        definition: &crate::runtime::subagent::NamedAgentDefinition,
        runs: Dependencies,
        skills: BTreeMap<String, Option<crate::runtime::identity::SkillVersionId>>,
    ) -> Self {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        for file in
            crate::runtime::subagent::resolver::resolve_project_instructions(definition, snapshot)
        {
            hasher.update(
                serde_json::to_vec(&(file.path.display().to_string(), &file.content))
                    .expect("project instructions serialize"),
            );
        }
        Self {
            definition: definition.digest().clone(),
            runs,
            skills,
            project_instructions: format!("sha256:{:x}", hasher.finalize()),
            environment: Environment::of(snapshot),
        }
    }

    /// An Agent `subagent` delegates with no override, from the profile the
    /// generation resolved for it.
    fn delegated(
        snapshot: &RuntimeResourceSnapshot,
        name: &crate::runtime::subagent::SubagentName,
    ) -> Option<Self> {
        use crate::capabilities::selection::AgentToolSelection;
        let profile = snapshot.resolved_agent(name)?;
        let sources = profile
            .tool_selection
            .iter()
            .filter_map(|selection| match selection {
                AgentToolSelection::Builtin { .. } => None,
                AgentToolSelection::Source { source_id, .. }
                | AgentToolSelection::All { source_id } => Some(source_id.clone()),
            })
            .chain(profile.resource_sources.iter().cloned());
        let versions = skill_versions(snapshot);
        Some(Self::new(
            snapshot,
            snapshot.subagents().get(name)?,
            Dependencies::new(snapshot, &profile.tools, sources),
            profile
                .skills
                .iter()
                .map(|skill| (skill.clone(), versions.get(skill.as_str()).cloned()))
                .collect(),
        ))
    }

    /// An Agent a Workflow froze at admission.
    fn frozen(
        snapshot: &RuntimeResourceSnapshot,
        frozen: &crate::runtime::subagent::resolver::FrozenAgentComposition,
    ) -> Self {
        Self::new(
            snapshot,
            frozen.definition(),
            Dependencies::new(
                snapshot,
                frozen.tool_definitions(),
                frozen.sources().cloned(),
            ),
            frozen
                .skills()
                .map(|(name, version)| (name.to_owned(), Some(version.clone())))
                .collect(),
        )
    }
}

/// One Skill as a commit compares it: its catalog entry and content version.
#[derive(Debug, PartialEq, serde::Serialize)]
struct SkillEntry {
    entry: crate::skills::SkillCatalogEntry,
    version_id: Option<crate::runtime::identity::SkillVersionId>,
}

fn skill_versions(
    snapshot: &RuntimeResourceSnapshot,
) -> BTreeMap<&str, crate::runtime::identity::SkillVersionId> {
    snapshot
        .capability()
        .skills()
        .bindings()
        .iter()
        .map(|binding| (binding.skill_id.as_str(), binding.version_id.clone()))
        .collect()
}

/// The enabled Workflow programs by the id of the Tool that runs each.
fn enabled_programs(
    snapshot: &RuntimeResourceSnapshot,
) -> BTreeMap<crate::runtime::identity::ToolId, &crate::runtime::workflow::WorkflowProgram> {
    snapshot
        .workflows()
        .entries()
        .iter()
        .filter_map(|(id, entry)| match &entry.admission {
            crate::runtime::workflow::WorkflowAdmission::Enabled(program) => {
                Some((crate::tools::native::workflow_tool_id(id), program.as_ref()))
            }
            crate::runtime::workflow::WorkflowAdmission::Disabled(_) => None,
        })
        .collect()
}

/// A Tool is its whole definition and its implementation identity: the
/// executable binding of the server it runs on, or the enabled Workflow
/// program it runs with the leaf Tools and Named Agents beneath it, or the
/// Named Agents `subagent` delegates. A same-name implementation change is
/// `changed` even when the model-visible declaration is byte-identical; an
/// Agent or source nothing runs changes no Tool.
fn tool_entries(snapshot: &RuntimeResourceSnapshot) -> BTreeMap<String, ToolEntry> {
    let capability = snapshot.capability();
    let registry = capability.tool_registry();
    let programs = enabled_programs(snapshot);
    registry
        .definitions()
        .into_iter()
        .zip(registry.model_definitions())
        .map(|(definition, model)| {
            let server = match &definition.origin {
                crate::tools::types::ToolOrigin::Builtin => None,
                crate::tools::types::ToolOrigin::Mcp { server_id } => Some(server_id.clone()),
                crate::tools::types::ToolOrigin::ManagedPython { package } => {
                    Some(crate::tools::python::python_server_id(package))
                }
            };
            let binding = server.and_then(|id| capability.mcp_servers().get(&id).cloned());
            let program = programs.get(&definition.id).copied();
            let (runs, agents) = match program {
                Some(program) => (
                    Dependencies::new(snapshot, program.frozen_tools(), []),
                    program
                        .agent_nodes()
                        .into_iter()
                        .filter_map(|agent| {
                            let frozen = agent.resolved.as_ref()?;
                            Some((
                                agent.profile.to_string(),
                                AgentEntry::frozen(snapshot, frozen),
                            ))
                        })
                        .collect(),
                ),
                None if definition.name == crate::tools::native::SUBAGENT_TOOL_NAME
                    && definition.origin == crate::tools::types::ToolOrigin::Builtin =>
                {
                    (
                        Dependencies::default(),
                        snapshot
                            .delegatable_agents()
                            .iter()
                            .filter_map(|name| {
                                Some((name.to_string(), AgentEntry::delegated(snapshot, name)?))
                            })
                            .collect(),
                    )
                }
                None => (Dependencies::default(), BTreeMap::new()),
            };
            (
                definition.name.clone(),
                ToolEntry {
                    model_input_schema: model.input_schema,
                    server_binding_digest: binding.as_ref().map(binding_digest),
                    binding,
                    workflow: program.map(crate::runtime::workflow::WorkflowProgram::tool_identity),
                    runs,
                    agents,
                    definition,
                },
            )
        })
        .collect()
}

/// Whether every model invocation the generation runs resolves to the same
/// execution — provider binding, wire model, compat and effective parameters
/// — on both sides: the Session's own selections, the explicit model of
/// each Named Agent a Tool runs and of each direct Workflow Model node.
/// Catalog entries nothing selects are not compared; a selection that
/// resolves on neither side is the Session view's to report.
fn same_invocations(
    (old, old_model): (&RuntimeResourceSnapshot, &SessionModelState),
    (new, new_model): (&RuntimeResourceSnapshot, &SessionModelState),
) -> bool {
    use crate::model::session::{analyze_session_model_config, same_session_invocation};
    let (Some(left), Some(right)) = (old_model.catalog(), new_model.catalog()) else {
        return true;
    };
    let same = |before: &crate::model::session::SessionModelConfig,
                after: &crate::model::session::SessionModelConfig| {
        same_session_invocation(left, before, right, after)
            || (analyze_session_model_config(left, before).is_err()
                && analyze_session_model_config(right, after).is_err())
    };
    let agent_models = |snapshot: &RuntimeResourceSnapshot| {
        let workflow_agents = enabled_programs(snapshot)
            .into_values()
            .flat_map(|program| {
                program
                    .agent_nodes()
                    .into_iter()
                    .map(|agent| agent.profile.clone())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        snapshot
            .delegatable_agents()
            .iter()
            .cloned()
            .chain(workflow_agents)
            .filter_map(|name| {
                let model = snapshot.subagents().get(&name)?.model()?.clone();
                Some((name, model))
            })
            .collect::<BTreeMap<_, _>>()
    };
    // Each explicit model a direct Model node of an enabled Workflow names,
    // at any depth, by its Workflow and authored path.
    let workflow_models = |snapshot: &RuntimeResourceSnapshot| {
        enabled_programs(snapshot)
            .into_iter()
            .flat_map(|(tool, program)| {
                program
                    .model_policy_nodes()
                    .into_iter()
                    .filter_map(|node| Some(((tool.clone(), node.path), node.policy.selection()?)))
                    .collect::<Vec<_>>()
            })
            .collect::<BTreeMap<_, _>>()
    };
    let resolves = |catalog: &crate::model::catalog::ModelCatalog,
                    selection: &crate::model::ModelSelection| {
        catalog.same_invocation(selection, catalog, selection, SESSION_LAYER)
    };
    let before = agent_models(old);
    let before_workflows = workflow_models(old);
    same(old_model.config(), new_model.config())
        && agent_models(new).iter().all(|(name, after)| {
            // An Agent newly run or newly naming a model changes its Tool.
            before.get(name).is_none_or(|before| same(before, after))
        })
        && workflow_models(new).iter().all(|(node, after)| {
            // A node newly naming a model changes its Workflow program.
            before_workflows.get(node).is_none_or(|before| {
                left.same_invocation(before, right, after, SESSION_LAYER)
                    || (!resolves(left, before) && !resolves(right, after))
            })
        })
}

const SESSION_LAYER: crate::model::invocation::RequestParamsLayer =
    crate::model::invocation::RequestParamsLayer::SessionOverrides;

/// A Skill is its catalog entry and its content-derived version, so a body or
/// script edit under unchanged frontmatter is `changed`.
fn skill_entries(snapshot: &RuntimeResourceSnapshot) -> BTreeMap<String, SkillEntry> {
    let versions = skill_versions(snapshot);
    snapshot
        .capability()
        .model_skill_entries()
        .iter()
        .map(|entry| {
            (
                entry.name.clone(),
                SkillEntry {
                    entry: entry.clone(),
                    version_id: versions.get(entry.name.as_str()).cloned(),
                },
            )
        })
        .collect()
}

/// Every item a commit affected, in name order, with the state it left
/// (`before`, for a change or removal) and the state it installed (`after`,
/// for an addition or change).
fn affected<T: PartialEq + serde::Serialize>(
    before: &BTreeMap<String, T>,
    after: &BTreeMap<String, T>,
) -> Vec<serde_json::Value> {
    before
        .keys()
        .chain(after.keys())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .filter_map(|name| {
            let (old, new) = (before.get(name), after.get(name));
            let change = match (old, new) {
                (None, Some(_)) => "added",
                (Some(_), None) => "removed",
                (Some(old), Some(new)) if old != new => "changed",
                _ => return None,
            };
            Some(serde_json::json!({
                "name": name,
                "change": change,
                "before": old,
                "after": new,
            }))
        })
        .collect()
}

/// A stable identity of what a server binding executes: its transport and
/// invocation policy. Credentials never enter it.
fn binding_digest(binding: &crate::tools::mcp::McpServerBinding) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    let mut hasher = Sha256::new();
    hasher.update(b"rustx/configuration/server-binding/v1\0");
    hasher.update(
        serde_json::to_vec(&(&binding.transport, &binding.policy))
            .expect("server binding serializes"),
    );
    let mut digest = String::from("sha256:");
    for byte in hasher.finalize() {
        let _ = write!(digest, "{byte:02x}");
    }
    digest
}

/// One coherent cut of a runtime's committed configuration: the generation
/// future Attempts acquire, the Session binding and model intent, the
/// effective approval mode, and the configuration the current Attempt froze.
pub(crate) struct ConfigurationCut {
    pub(crate) binding_revision: u64,
    pub(crate) resources: Arc<RuntimeResourceSnapshot>,
    pub(crate) approval_mode: crate::runtime::ApprovalMode,
    pub(crate) session_model: Option<crate::model::session::SessionModelConfig>,
    pub(crate) effective_model: crate::model::session::SessionModelView,
    pub(crate) admitted_attempt: Option<AdmittedConfiguration>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_name_definition_change_is_changed_not_removed_and_added() {
        let before = BTreeMap::from([("read".to_owned(), 1), ("write".to_owned(), 1)]);
        let after = BTreeMap::from([("read".to_owned(), 2), ("grep".to_owned(), 1)]);
        let changes = NamedChanges::between(&before, &after);
        assert_eq!(changes.added.names, ["grep"]);
        assert_eq!(changes.removed.names, ["write"]);
        assert_eq!(changes.changed.names, ["read"]);
        assert!(NamedChanges::between(&after, &after).is_empty());
    }

    #[test]
    fn name_lists_are_bounded_with_exact_totals() {
        let after = (0..100)
            .map(|index| (format!("tool_{index:03}"), 0_u8))
            .collect::<BTreeMap<_, _>>();
        let changes = NamedChanges::between(&BTreeMap::new(), &after);
        assert_eq!(changes.added.total, 100);
        assert_eq!(changes.added.names.len(), CONFIGURATION_CHANGE_NAME_LIMIT);
        assert_eq!(changes.added.names[0], "tool_000");
    }

    #[test]
    fn name_lists_are_bounded_by_encoded_bytes() {
        let long = "x".repeat(20 * 1024);
        let escaped = "\"\u{1}水".repeat(200);
        let after = BTreeMap::from([
            (long.clone(), 0_u8),
            (escaped.clone(), 0),
            ("a".to_owned(), 0),
        ]);
        let changes = NamedChanges::between(&BTreeMap::new(), &after);
        assert_eq!(changes.added.total, 3);
        // Sorted, so the short name is kept and the oversized ones are counted.
        assert_eq!(changes.added.names, ["a"]);
        let change = ConfigurationChange {
            reason: ConfigurationChangeReason::Source,
            previous_binding_revision: 1,
            binding_revision: 2,
            domains: vec![ConfigurationDomain::Tools, ConfigurationDomain::Skills],
            tools: NamedChanges {
                added: changes.added.clone(),
                removed: NamedChanges::between(&after, &BTreeMap::new()).removed,
                changed: changes.added.clone(),
            },
            skills: NamedChanges {
                added: changes.added.clone(),
                removed: changes.added.clone(),
                changed: changes.added,
            },
            instructions_bytes: 0,
            catalog_bytes: 0,
        };
        let worst = (0..CONFIGURATION_CHANGE_NAME_LIMIT)
            .map(|index| (format!("{index:02}{}", "\"".repeat(200)), 0_u8))
            .collect::<BTreeMap<_, _>>();
        let worst = NamedChanges::between(&BTreeMap::new(), &worst).added;
        assert!(
            serde_json::to_string(&worst.names).unwrap().len()
                <= CONFIGURATION_CHANGE_NAME_BYTES + 2
        );
        let full = ConfigurationChange {
            tools: NamedChanges {
                added: worst.clone(),
                removed: worst.clone(),
                changed: worst.clone(),
            },
            skills: NamedChanges {
                added: worst.clone(),
                removed: worst.clone(),
                changed: worst,
            },
            ..change.clone()
        };
        for change in [change, full] {
            assert!(serde_json::to_vec(&change).unwrap().len() < 8 * 1024);
        }
    }

    #[test]
    fn a_superseded_desire_is_never_current_again() {
        let current = Arc::new(AtomicU64::new(2));
        let v2 = DesiredApplication::new(current.clone(), 2);
        assert!(v2.is_current());
        current.store(3, Ordering::SeqCst);
        assert!(!v2.is_current());
        assert!(DesiredApplication::new(current, 3).is_current());
    }
}
