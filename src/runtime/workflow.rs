//! Native, provider-independent Workflow programs (Issue #83).
//!
//! YAML is only the serialization format at this boundary. The loader turns a
//! configured workflow file into [`WorkflowDefinition`], the compiler checks
//! the finite graph and every explicit value reference, and the immutable
//! [`WorkflowProgram`] is the only representation execution code consumes.
//! Dynamic ownership and orchestration state belong to `WorkflowRuntime`;
//! child model execution remains in the native subagent runtime.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::sync::Arc;

use chrono::Utc;
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::durable::ConversationStore;
use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
use crate::runtime::identity::{EventId, SubagentId, ToolCallId};

use super::subagent::SubagentName;

/// The maximum serialized workflow size accepted by the native loader.
pub const MAX_WORKFLOW_BYTES: usize = 512 * 1024;
/// The maximum number of nodes in one workflow program.
pub const MAX_WORKFLOW_NODES: usize = 256;
/// The maximum number of registered workflow definitions in one generation.
pub const MAX_WORKFLOW_DEFINITIONS: usize = 64;
/// The maximum number of explicit parallel branches in one node.
pub const MAX_PARALLEL_BRANCHES: usize = 32;
/// The maximum number of path components in one explicit reference.
pub const MAX_REFERENCE_COMPONENTS: usize = 32;
/// The reserved model-facing name consumed by a Workflow-owned `AgentRun`'s
/// terminal protocol. A Workflow cannot claim this name as a parent Tool.
pub const WORKFLOW_OUTPUT_TOOL_NAME: &str = "workflow_output";
/// The reserved Tool-id namespace of model-facing Workflow Tools.
///
/// Workflow Tools are concrete parent-plane capabilities, but they are not
/// selectable child capabilities: rejecting this namespace at the named
/// Subagent resolution boundary keeps Workflow-to-Workflow composition out
/// of the v1 language and child materialization path.
pub const WORKFLOW_TOOL_ID_PREFIX: &str = "tool-workflow-";

/// The configured workflow identity.
///
/// This is both the catalog key and the eventual model-facing Tool name. It
/// is deliberately not repeated inside YAML.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct WorkflowId(String);

impl WorkflowId {
    /// Parses one configured workflow identity.
    ///
    /// # Errors
    ///
    /// Returns [`WorkflowIdError`] when the identity is empty, too long, or
    /// contains a character outside the bounded lowercase id alphabet.
    pub fn parse(value: &str) -> Result<Self, WorkflowIdError> {
        if value.is_empty() {
            return Err(WorkflowIdError::Empty);
        }
        if value.len() > 64 {
            return Err(WorkflowIdError::TooLong(value.len()));
        }
        let Some(first) = value.chars().next() else {
            return Err(WorkflowIdError::Empty);
        };
        if !first.is_ascii_lowercase() {
            return Err(WorkflowIdError::InvalidCharacter(first));
        }
        if let Some(found) = value
            .chars()
            .find(|character| !matches!(character, 'a'..='z' | '0'..='9' | '-' | '_'))
        {
            return Err(WorkflowIdError::InvalidCharacter(found));
        }
        Ok(Self(value.to_owned()))
    }

    /// The canonical identity text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for WorkflowId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for WorkflowId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A workflow identity violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowIdError {
    /// The identity is empty.
    Empty,
    /// The identity exceeds the bounded key size.
    TooLong(usize),
    /// The identity contains an unsupported character.
    InvalidCharacter(char),
}

impl fmt::Display for WorkflowIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("workflow id must not be empty"),
            Self::TooLong(bytes) => write!(formatter, "workflow id is too long ({bytes} bytes)"),
            Self::InvalidCharacter(character) => write!(
                formatter,
                "workflow id accepts lowercase [a-z0-9_-] and found {character:?}"
            ),
        }
    }
}

impl std::error::Error for WorkflowIdError {}

/// The canonical YAML/domain representation of one workflow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDefinition {
    /// The model-facing description of the workflow Tool.
    pub description: String,
    /// The workflow input JSON Schema.
    pub input: Value,
    /// The workflow output JSON Schema.
    pub output: Value,
    /// The one explicit entry node.
    pub entry: String,
    /// Stable node definitions keyed by explicit node id.
    #[serde(deserialize_with = "deserialize_unique_map")]
    pub nodes: BTreeMap<String, WorkflowNodeDefinition>,
    /// Ordinary sequential control-flow edges.
    #[serde(default)]
    pub edges: Vec<WorkflowEdgeDefinition>,
}

/// One Workflow node in the authoring/domain layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum WorkflowNodeDefinition {
    /// One execution of an admitted named Subagent profile.
    Agent {
        /// The native named profile to resolve at `AgentRun` admission.
        profile: SubagentName,
        /// The fixed task instruction for this `AgentRun`.
        task: String,
        /// Explicit input bindings from workflow-local values.
        #[serde(default)]
        input: BTreeMap<String, WorkflowBinding>,
        /// The frozen `AgentRun` output contract.
        output: Value,
    },
    /// Deterministic selection from one committed boolean value.
    Branch {
        /// The sole boolean condition binding.
        condition: WorkflowBinding,
    },
    /// A finite keyed set of one-Agent branches.
    Parallel {
        /// Branches are keyed by definition identity, not completion order.
        #[serde(deserialize_with = "deserialize_unique_map")]
        branches: BTreeMap<String, WorkflowParallelBranchDefinition>,
    },
    /// Resolves explicit bindings and settles the workflow.
    Return {
        /// The fields of the workflow result.
        output: BTreeMap<String, WorkflowBinding>,
    },
}

/// One explicit structured workflow value reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowBinding {
    /// A path such as `args.task` or `review.blockers`.
    #[serde(rename = "ref")]
    pub reference: String,
}

/// One explicit edge in the workflow graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowEdgeDefinition {
    /// Source node id.
    pub from: String,
    /// Destination node id.
    pub to: String,
    /// Branch port. Non-Branch nodes must omit it.
    #[serde(default)]
    pub port: Option<WorkflowPort>,
}

/// A control-flow port of a Branch node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WorkflowPort {
    /// The true successor.
    True,
    /// The false successor.
    False,
    /// The ordinary single-successor port.
    Next,
}

impl Serialize for WorkflowPort {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(match self {
            Self::True => "true",
            Self::False => "false",
            Self::Next => "next",
        })
    }
}

impl<'de> Deserialize<'de> for WorkflowPort {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum RawPort {
            Boolean(bool),
            Text(String),
        }
        match RawPort::deserialize(deserializer)? {
            RawPort::Boolean(true) => Ok(Self::True),
            RawPort::Boolean(false) => Ok(Self::False),
            RawPort::Text(text) => match text.as_str() {
                "true" => Ok(Self::True),
                "false" => Ok(Self::False),
                "next" => Ok(Self::Next),
                _ => Err(serde::de::Error::custom(format!(
                    "unknown workflow edge port {text:?}"
                ))),
            },
        }
    }
}

/// One Agent branch inside a Parallel node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowParallelBranchDefinition {
    /// The native named profile.
    pub profile: SubagentName,
    /// The fixed task instruction.
    pub task: String,
    /// Explicit inputs.
    #[serde(default)]
    pub input: BTreeMap<String, WorkflowBinding>,
    /// The frozen branch output contract.
    pub output: Value,
}

/// A compiled, immutable executable workflow.
#[derive(Debug, Clone)]
pub struct WorkflowProgram {
    id: WorkflowId,
    description: String,
    input_schema: Value,
    output_schema: Value,
    entry: String,
    nodes: BTreeMap<String, WorkflowNodeProgram>,
    outgoing: BTreeMap<String, Vec<WorkflowEdgeProgram>>,
}

impl WorkflowProgram {
    /// Compiles and validates one definition for one configured identity.
    ///
    /// # Errors
    ///
    /// Returns a [`WorkflowCompileError`] when the definition's graph,
    /// schemas, references, or admitted profiles are invalid.
    pub fn compile(
        id: WorkflowId,
        definition: WorkflowDefinition,
        workflow_profiles: &BTreeSet<SubagentName>,
    ) -> Result<Self, WorkflowCompileError> {
        compile_program(id, definition, workflow_profiles)
    }

    /// The configured identity.
    #[must_use]
    pub fn id(&self) -> &WorkflowId {
        &self.id
    }

    /// The model-facing description.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// The immutable input schema.
    #[must_use]
    pub fn input_schema(&self) -> &Value {
        &self.input_schema
    }

    /// The immutable output schema.
    #[must_use]
    pub fn output_schema(&self) -> &Value {
        &self.output_schema
    }

    /// The explicit entry node.
    #[must_use]
    pub fn entry(&self) -> &str {
        &self.entry
    }

    /// The compiled nodes in deterministic id order.
    #[must_use]
    pub fn nodes(&self) -> &BTreeMap<String, WorkflowNodeProgram> {
        &self.nodes
    }

    /// The compiled outgoing edges of a node.
    #[must_use]
    pub fn outgoing(&self, node: &str) -> &[WorkflowEdgeProgram] {
        self.outgoing.get(node).map_or(&[], Vec::as_slice)
    }
}

/// A compiled node whose references and schemas have been admitted.
#[derive(Debug, Clone)]
pub enum WorkflowNodeProgram {
    /// One admitted `AgentRun` template.
    Agent(WorkflowAgentProgram),
    /// One boolean Branch.
    Branch { condition: WorkflowBinding },
    /// One keyed finite fan-out of `AgentRun` templates.
    Parallel {
        branches: BTreeMap<String, WorkflowAgentProgram>,
        output_schema: Value,
    },
    /// The terminal Return operation.
    Return {
        output: BTreeMap<String, WorkflowBinding>,
    },
}

/// A compiled `AgentRun` template.
#[derive(Debug, Clone)]
pub struct WorkflowAgentProgram {
    /// The admitted native profile.
    pub profile: SubagentName,
    /// The fixed task string.
    pub task: String,
    /// Explicit input bindings.
    pub input: BTreeMap<String, WorkflowBinding>,
    /// Frozen output contract.
    pub output_schema: Value,
}

/// A compiled edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowEdgeProgram {
    /// Destination id.
    pub to: String,
    /// The port used by this edge.
    pub port: WorkflowPort,
}

/// An immutable registered workflow catalog.
#[derive(Debug, Clone, Default)]
pub struct WorkflowCatalog {
    definitions: BTreeMap<WorkflowId, Arc<WorkflowProgram>>,
    main: BTreeSet<WorkflowId>,
}

impl WorkflowCatalog {
    /// Creates a catalog and validates the model-visible admission subset.
    ///
    /// # Errors
    ///
    /// Returns a [`WorkflowCatalogError`] for duplicate definitions,
    /// duplicate model-visible ids, or an unknown model-visible id.
    pub fn new(
        programs: impl IntoIterator<Item = WorkflowProgram>,
        main: impl IntoIterator<Item = WorkflowId>,
    ) -> Result<Self, WorkflowCatalogError> {
        let mut definitions = BTreeMap::new();
        for program in programs {
            if definitions
                .insert(program.id.clone(), Arc::new(program))
                .is_some()
            {
                return Err(WorkflowCatalogError::DuplicateDefinition);
            }
        }
        let mut admitted_main = BTreeSet::new();
        for id in main {
            if !admitted_main.insert(id.clone()) {
                return Err(WorkflowCatalogError::DuplicateMain(id));
            }
        }
        let main = admitted_main;
        if let Some(unknown) = main.iter().find(|id| !definitions.contains_key(*id)) {
            return Err(WorkflowCatalogError::UnknownMain(unknown.clone()));
        }
        Ok(Self { definitions, main })
    }

    /// The empty catalog.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Looks up a registered immutable program.
    #[must_use]
    pub fn get(&self, id: &WorkflowId) -> Option<&Arc<WorkflowProgram>> {
        self.definitions.get(id)
    }

    /// All registered programs in identity order.
    #[must_use]
    pub fn definitions(&self) -> &BTreeMap<WorkflowId, Arc<WorkflowProgram>> {
        &self.definitions
    }

    /// The explicitly model-visible workflow ids.
    #[must_use]
    pub fn main(&self) -> &BTreeSet<WorkflowId> {
        &self.main
    }

    /// Whether this catalog has no registered definitions.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }
}

/// A workflow catalog admission failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowCatalogError {
    /// Two compiled programs used one configured identity.
    DuplicateDefinition,
    /// A model-visible id is not registered.
    UnknownMain(WorkflowId),
    /// A model-visible id was repeated in the admission list.
    DuplicateMain(WorkflowId),
}

impl fmt::Display for WorkflowCatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateDefinition => formatter.write_str("duplicate workflow definition id"),
            Self::UnknownMain(id) => {
                write!(formatter, "workflows.main names unknown workflow {id:?}")
            }
            Self::DuplicateMain(id) => {
                write!(formatter, "workflows.main repeats workflow {id:?}")
            }
        }
    }
}

impl std::error::Error for WorkflowCatalogError {}

/// A compile-time graph/type/reference rejection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowCompileError {
    /// A required textual field is empty or over its bound.
    InvalidField(String),
    /// A JSON Schema is invalid or not a root object schema.
    InvalidSchema(String),
    /// A node or edge refers to an unknown id.
    DanglingReference(String),
    /// The graph has no single explicit entry.
    InvalidEntry(String),
    /// The graph contains a cycle.
    Cycle,
    /// A node is unreachable from the explicit entry.
    Unreachable(String),
    /// A path can leave a node without a deterministic successor/terminal.
    Unterminated(String),
    /// A Branch does not provide exactly one true and false successor.
    InvalidBranch(String),
    /// A workflow reference cannot be resolved on every relevant path.
    InvalidReference(String),
    /// A reference has an incompatible statically known schema.
    IncompatibleReference(String),
    /// An Agent profile is not workflow-admitted.
    ProfileNotAdmitted(SubagentName),
}

impl fmt::Display for WorkflowCompileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidField(detail)
            | Self::InvalidSchema(detail)
            | Self::DanglingReference(detail)
            | Self::InvalidEntry(detail)
            | Self::Unterminated(detail)
            | Self::InvalidBranch(detail)
            | Self::InvalidReference(detail)
            | Self::IncompatibleReference(detail) => formatter.write_str(detail),
            Self::Cycle => formatter.write_str("workflow graph contains a cycle"),
            Self::Unreachable(node) => write!(formatter, "workflow node {node:?} is unreachable"),
            Self::ProfileNotAdmitted(profile) => write!(
                formatter,
                "workflow Agent profile {profile:?} is not admitted by subagents.workflow"
            ),
        }
    }
}

impl std::error::Error for WorkflowCompileError {}

#[allow(clippy::too_many_lines)] // one bounded graph-to-program validation pipeline
fn compile_program(
    id: WorkflowId,
    definition: WorkflowDefinition,
    workflow_profiles: &BTreeSet<SubagentName>,
) -> Result<WorkflowProgram, WorkflowCompileError> {
    if definition.description.trim().is_empty() {
        return Err(WorkflowCompileError::InvalidField(format!(
            "workflow {id} has an empty description"
        )));
    }
    if definition.description.len() > 4096 {
        return Err(WorkflowCompileError::InvalidField(format!(
            "workflow {id} description is too large"
        )));
    }
    validate_root_schema(&definition.input, "input")?;
    validate_root_schema(&definition.output, "output")?;
    if definition.nodes.is_empty() || definition.nodes.len() > MAX_WORKFLOW_NODES {
        return Err(WorkflowCompileError::InvalidField(format!(
            "workflow {id} must contain between one and {MAX_WORKFLOW_NODES} nodes"
        )));
    }
    if definition.entry.trim().is_empty() {
        return Err(WorkflowCompileError::InvalidEntry(
            "workflow entry must be explicit and non-empty".to_owned(),
        ));
    }
    for node_id in definition.nodes.keys() {
        if node_id.trim().is_empty() || node_id.len() > 64 || node_id.contains('.') {
            return Err(WorkflowCompileError::InvalidField(format!(
                "workflow node id {node_id:?} must be non-empty, at most 64 bytes, and contain no dots"
            )));
        }
    }
    if !definition.nodes.contains_key(&definition.entry) {
        return Err(WorkflowCompileError::InvalidEntry(format!(
            "workflow entry {:?} does not name a node",
            definition.entry
        )));
    }

    let mut outgoing: BTreeMap<String, Vec<WorkflowEdgeProgram>> = definition
        .nodes
        .keys()
        .cloned()
        .map(|node| (node, Vec::new()))
        .collect();
    let mut incoming: BTreeMap<String, usize> = definition
        .nodes
        .keys()
        .cloned()
        .map(|node| (node, 0))
        .collect();
    for edge in &definition.edges {
        if !definition.nodes.contains_key(&edge.from) {
            return Err(WorkflowCompileError::DanglingReference(format!(
                "edge source {:?} is not a workflow node",
                edge.from
            )));
        }
        if !definition.nodes.contains_key(&edge.to) {
            return Err(WorkflowCompileError::DanglingReference(format!(
                "edge destination {:?} is not a workflow node",
                edge.to
            )));
        }
        let node = definition.nodes.get(&edge.from).expect("checked above");
        let port = match (node, edge.port) {
            (WorkflowNodeDefinition::Branch { .. }, Some(WorkflowPort::True)) => WorkflowPort::True,
            (WorkflowNodeDefinition::Branch { .. }, Some(WorkflowPort::False)) => {
                WorkflowPort::False
            }
            (WorkflowNodeDefinition::Branch { .. }, _) => {
                return Err(WorkflowCompileError::InvalidBranch(format!(
                    "Branch node {:?} must use true and false ports",
                    edge.from
                )));
            }
            (_, None) => WorkflowPort::Next,
            (_, Some(port)) => {
                return Err(WorkflowCompileError::InvalidField(format!(
                    "non-Branch node {:?} cannot have port {port:?}",
                    edge.from
                )));
            }
        };
        outgoing
            .get_mut(&edge.from)
            .expect("node exists")
            .push(WorkflowEdgeProgram {
                to: edge.to.clone(),
                port,
            });
        *incoming.get_mut(&edge.to).expect("node exists") += 1;
    }
    for edges in outgoing.values_mut() {
        edges.sort_by(|left, right| left.port.cmp(&right.port).then(left.to.cmp(&right.to)));
        if edges.windows(2).any(|pair| pair[0].port == pair[1].port) {
            return Err(WorkflowCompileError::InvalidBranch(
                "a node has duplicate control-flow ports".to_owned(),
            ));
        }
    }
    if incoming[&definition.entry] != 0 {
        return Err(WorkflowCompileError::InvalidEntry(format!(
            "entry node {:?} must not have an incoming edge",
            definition.entry
        )));
    }
    let topological = topological_order(&outgoing, &incoming)?;
    let reachable = reachable_nodes(&definition.entry, &outgoing);
    if let Some(unreachable) = definition
        .nodes
        .keys()
        .find(|node| !reachable.contains(*node))
    {
        return Err(WorkflowCompileError::Unreachable(unreachable.clone()));
    }

    let mut nodes = BTreeMap::new();
    let mut available_after: BTreeMap<String, SchemaMap> = BTreeMap::new();
    let args_schema = SchemaMap::from_schema(&definition.input);
    for node_id in topological {
        let node = definition
            .nodes
            .get(&node_id)
            .expect("topological node exists");
        let available_before = if node_id == definition.entry {
            args_schema.clone()
        } else {
            let predecessors = definition
                .nodes
                .keys()
                .filter(|candidate| {
                    outgoing[candidate.as_str()]
                        .iter()
                        .any(|edge| edge.to == node_id)
                })
                .filter_map(|candidate| available_after.get(candidate))
                .collect::<Vec<_>>();
            intersect_schema_maps(&predecessors)
        };
        let compiled = match node {
            WorkflowNodeDefinition::Agent {
                profile,
                task,
                input,
                output,
            } => {
                validate_agent(
                    profile,
                    task,
                    input,
                    output,
                    workflow_profiles,
                    &available_before,
                    &node_id,
                )?;
                let agent = WorkflowAgentProgram {
                    profile: profile.clone(),
                    task: task.clone(),
                    input: input.clone(),
                    output_schema: output.clone(),
                };
                available_after.insert(
                    node_id.clone(),
                    available_before.with_prefix(&node_id, &SchemaMap::from_schema(output)),
                );
                WorkflowNodeProgram::Agent(agent)
            }
            WorkflowNodeDefinition::Branch { condition } => {
                let schema = resolve_reference(condition, &available_before, &node_id)?;
                if schema_type(schema) != Some("boolean") {
                    return Err(WorkflowCompileError::IncompatibleReference(format!(
                        "Branch {:?} condition {} must resolve to boolean",
                        node_id, condition.reference
                    )));
                }
                if outgoing[&node_id].len() != 2
                    || !outgoing[&node_id]
                        .iter()
                        .any(|edge| edge.port == WorkflowPort::True)
                    || !outgoing[&node_id]
                        .iter()
                        .any(|edge| edge.port == WorkflowPort::False)
                {
                    return Err(WorkflowCompileError::InvalidBranch(format!(
                        "Branch {node_id:?} must have exactly true and false successors"
                    )));
                }
                available_after.insert(node_id.clone(), available_before.clone());
                WorkflowNodeProgram::Branch {
                    condition: condition.clone(),
                }
            }
            WorkflowNodeDefinition::Parallel { branches } => {
                if branches.is_empty() || branches.len() > MAX_PARALLEL_BRANCHES {
                    return Err(WorkflowCompileError::InvalidField(format!(
                        "Parallel {node_id:?} must contain between one and {MAX_PARALLEL_BRANCHES} branches"
                    )));
                }
                let mut compiled_branches = BTreeMap::new();
                let mut output_properties = serde_json::Map::new();
                for (key, branch) in branches {
                    if key.trim().is_empty() || key.len() > 64 || key.contains('.') {
                        return Err(WorkflowCompileError::InvalidField(format!(
                            "Parallel {node_id:?} branch key {key:?} must be non-empty, at most 64 bytes, and contain no dots"
                        )));
                    }
                    validate_agent(
                        &branch.profile,
                        &branch.task,
                        &branch.input,
                        &branch.output,
                        workflow_profiles,
                        &available_before,
                        &format!("{node_id}.{key}"),
                    )?;
                    output_properties.insert(key.clone(), branch.output.clone());
                    compiled_branches.insert(
                        key.clone(),
                        WorkflowAgentProgram {
                            profile: branch.profile.clone(),
                            task: branch.task.clone(),
                            input: branch.input.clone(),
                            output_schema: branch.output.clone(),
                        },
                    );
                }
                let output_schema = serde_json::json!({
                    "type": "object",
                    "properties": output_properties,
                    "required": branches.keys().collect::<Vec<_>>(),
                    "additionalProperties": false
                });
                available_after.insert(
                    node_id.clone(),
                    available_before.with_prefix(&node_id, &SchemaMap::from_schema(&output_schema)),
                );
                WorkflowNodeProgram::Parallel {
                    branches: compiled_branches,
                    output_schema,
                }
            }
            WorkflowNodeDefinition::Return { output } => {
                if !outgoing[&node_id].is_empty() {
                    return Err(WorkflowCompileError::Unterminated(format!(
                        "Return node {node_id:?} cannot have outgoing edges"
                    )));
                }
                validate_return(output, &available_before, &definition.output, &node_id)?;
                available_after.insert(node_id.clone(), available_before.clone());
                WorkflowNodeProgram::Return {
                    output: output.clone(),
                }
            }
        };
        let edges = &outgoing[&node_id];
        match &compiled {
            WorkflowNodeProgram::Return { .. } => {}
            WorkflowNodeProgram::Branch { .. } if edges.len() == 2 => {}
            WorkflowNodeProgram::Branch { .. } => {
                return Err(WorkflowCompileError::InvalidBranch(format!(
                    "Branch {node_id:?} must have exactly two successors"
                )));
            }
            _ if edges.len() != 1 => {
                return Err(WorkflowCompileError::Unterminated(format!(
                    "node {node_id:?} must have exactly one successor"
                )));
            }
            _ => {}
        }
        nodes.insert(node_id, compiled);
    }
    if !nodes
        .values()
        .any(|node| matches!(node, WorkflowNodeProgram::Return { .. }))
    {
        return Err(WorkflowCompileError::Unterminated(
            "every workflow must contain a reachable Return node".to_owned(),
        ));
    }
    for node in nodes.keys() {
        if !reaches_return(node, &outgoing, &nodes, &mut BTreeSet::new()) {
            return Err(WorkflowCompileError::Unterminated(node.clone()));
        }
    }
    Ok(WorkflowProgram {
        id,
        description: definition.description,
        input_schema: definition.input,
        output_schema: definition.output,
        entry: definition.entry,
        nodes,
        outgoing,
    })
}

/// Deserializes a bounded keyed map without allowing a later YAML/JSON key to
/// silently replace an earlier one. Stable node and parallel-branch identity
/// is part of the `WorkflowDefinition` contract, so duplicate keys are a
/// definition error rather than a parser-specific last-write-wins detail.
fn deserialize_unique_map<'de, D, K, V>(deserializer: D) -> Result<BTreeMap<K, V>, D::Error>
where
    D: Deserializer<'de>,
    K: Deserialize<'de> + Ord,
    V: Deserialize<'de>,
{
    struct UniqueMapVisitor<K, V>(std::marker::PhantomData<(K, V)>);

    impl<'de, K, V> Visitor<'de> for UniqueMapVisitor<K, V>
    where
        K: Deserialize<'de> + Ord,
        V: Deserialize<'de>,
    {
        type Value = BTreeMap<K, V>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a map with unique keys")
        }

        fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
            let mut values = BTreeMap::new();
            while let Some(key) = access.next_key::<K>()? {
                if values.contains_key(&key) {
                    return Err(serde::de::Error::custom("duplicate map key"));
                }
                let value = access.next_value::<V>()?;
                values.insert(key, value);
            }
            Ok(values)
        }
    }

    deserializer.deserialize_map(UniqueMapVisitor(std::marker::PhantomData))
}

fn validate_root_schema(schema: &Value, label: &str) -> Result<(), WorkflowCompileError> {
    let Some(object) = schema.as_object() else {
        return Err(WorkflowCompileError::InvalidSchema(format!(
            "workflow {label} schema must be an object"
        )));
    };
    if object.get("type").and_then(Value::as_str) != Some("object") {
        return Err(WorkflowCompileError::InvalidSchema(format!(
            "workflow {label} schema must have root type object"
        )));
    }
    jsonschema::Validator::new(schema).map_err(|error| {
        WorkflowCompileError::InvalidSchema(format!("workflow {label}: {error}"))
    })?;
    Ok(())
}

fn validate_agent(
    profile: &SubagentName,
    task: &str,
    input: &BTreeMap<String, WorkflowBinding>,
    output: &Value,
    workflow_profiles: &BTreeSet<SubagentName>,
    available: &SchemaMap,
    node: &str,
) -> Result<(), WorkflowCompileError> {
    if !workflow_profiles.contains(profile) {
        return Err(WorkflowCompileError::ProfileNotAdmitted(profile.clone()));
    }
    if task.trim().is_empty() || task.len() > 32 * 1024 {
        return Err(WorkflowCompileError::InvalidField(format!(
            "Agent {node:?} task must be a fixed non-empty string within its bound"
        )));
    }
    if task.contains("${") || task.contains("{{") {
        return Err(WorkflowCompileError::InvalidField(format!(
            "Agent {node:?} task cannot contain interpolation syntax"
        )));
    }
    validate_root_schema(output, &format!("Agent {node} output"))?;
    for (name, binding) in input {
        if name.trim().is_empty() {
            return Err(WorkflowCompileError::InvalidField(format!(
                "Agent {node:?} has an empty input binding name"
            )));
        }
        if name.len() > 64 || name.contains('.') {
            return Err(WorkflowCompileError::InvalidField(format!(
                "Agent {node:?} input binding name {name:?} must be at most 64 bytes and contain no dots"
            )));
        }
        resolve_reference(binding, available, node)?;
    }
    Ok(())
}

fn validate_return(
    bindings: &BTreeMap<String, WorkflowBinding>,
    available: &SchemaMap,
    output_schema: &Value,
    node: &str,
) -> Result<(), WorkflowCompileError> {
    let properties = output_schema
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let required = output_schema
        .get("required")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    for key in &required {
        if !bindings.contains_key(*key) {
            return Err(WorkflowCompileError::InvalidReference(format!(
                "Return {node:?} does not bind required output field {key:?}"
            )));
        }
    }
    for (key, binding) in bindings {
        let Some(expected) = properties.get(key) else {
            return Err(WorkflowCompileError::InvalidReference(format!(
                "Return {node:?} binds unknown workflow output field {key:?}"
            )));
        };
        let actual = resolve_reference(binding, available, node)?;
        if !schemas_compatible(actual, expected) {
            return Err(WorkflowCompileError::IncompatibleReference(format!(
                "Return {node:?} binding {} is incompatible with output field {key:?}",
                binding.reference
            )));
        }
    }
    Ok(())
}

fn topological_order(
    outgoing: &BTreeMap<String, Vec<WorkflowEdgeProgram>>,
    incoming: &BTreeMap<String, usize>,
) -> Result<Vec<String>, WorkflowCompileError> {
    let mut counts = incoming.clone();
    let mut queue = incoming
        .iter()
        .filter_map(|(node, count)| (*count == 0).then_some(node.clone()))
        .collect::<VecDeque<_>>();
    let mut order = Vec::with_capacity(outgoing.len());
    while let Some(node) = queue.pop_front() {
        order.push(node.clone());
        for edge in &outgoing[&node] {
            let count = counts.get_mut(&edge.to).expect("edge destination exists");
            *count -= 1;
            if *count == 0 {
                queue.push_back(edge.to.clone());
            }
        }
    }
    if order.len() != outgoing.len() {
        return Err(WorkflowCompileError::Cycle);
    }
    Ok(order)
}

fn reachable_nodes(
    entry: &str,
    outgoing: &BTreeMap<String, Vec<WorkflowEdgeProgram>>,
) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::from([entry.to_owned()]);
    while let Some(node) = queue.pop_front() {
        if !seen.insert(node.clone()) {
            continue;
        }
        queue.extend(outgoing[&node].iter().map(|edge| edge.to.clone()));
    }
    seen
}

fn reaches_return(
    node: &str,
    outgoing: &BTreeMap<String, Vec<WorkflowEdgeProgram>>,
    programs: &BTreeMap<String, WorkflowNodeProgram>,
    visiting: &mut BTreeSet<String>,
) -> bool {
    if matches!(programs[node], WorkflowNodeProgram::Return { .. }) {
        return true;
    }
    if !visiting.insert(node.to_owned()) {
        return false;
    }
    let result = outgoing[node]
        .iter()
        .all(|edge| reaches_return(&edge.to, outgoing, programs, visiting));
    visiting.remove(node);
    result
}

#[derive(Debug, Clone, Default)]
struct SchemaMap(BTreeMap<String, Value>);

impl SchemaMap {
    fn from_schema(schema: &Value) -> Self {
        let mut map = BTreeMap::new();
        let required = schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<BTreeSet<_>>();
        if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
            for (name, property) in properties {
                if required.contains(name.as_str()) {
                    map.insert(name.clone(), property.clone());
                }
            }
        }
        Self(map)
    }

    fn with_prefix(&self, prefix: &str, schema: &Self) -> Self {
        let required = schema.0.keys().cloned().collect::<Vec<_>>();
        let mut result = self.clone();
        result.0.insert(
            prefix.to_owned(),
            serde_json::json!({
                "type": "object",
                "properties": schema.0,
                "required": required,
                "additionalProperties": true
            }),
        );
        result
    }
}

fn intersect_schema_maps(maps: &[&SchemaMap]) -> SchemaMap {
    let Some(first) = maps.first() else {
        return SchemaMap::default();
    };
    let mut result = (*first).clone();
    for map in &maps[1..] {
        result.0.retain(|key, schema| {
            map.0
                .get(key)
                .is_some_and(|other| schemas_equivalent_enough(schema, other))
        });
    }
    result
}

fn resolve_reference<'a>(
    binding: &WorkflowBinding,
    available: &'a SchemaMap,
    node: &str,
) -> Result<&'a Value, WorkflowCompileError> {
    let parts = binding.reference.split('.').collect::<Vec<_>>();
    if parts.is_empty()
        || parts.len() > MAX_REFERENCE_COMPONENTS
        || parts.iter().any(|part| part.is_empty())
    {
        return Err(WorkflowCompileError::InvalidReference(format!(
            "node {node:?} has malformed workflow reference {:?}",
            binding.reference
        )));
    }
    if parts[0] == "args" {
        if parts.len() < 2 {
            return Err(WorkflowCompileError::InvalidReference(format!(
                "workflow reference {:?} must name an input/output field",
                binding.reference
            )));
        }
        let Some(mut schema) = available.0.get(parts[1]) else {
            return Err(WorkflowCompileError::InvalidReference(format!(
                "node {node:?} references unavailable value {:?}",
                binding.reference
            )));
        };
        for part in &parts[2..] {
            let required = schema_required(schema).contains(part);
            if !required {
                return Err(WorkflowCompileError::InvalidReference(format!(
                    "node {node:?} reference {:?} crosses an optional field {:?}",
                    binding.reference, part
                )));
            }
            let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
                return Err(WorkflowCompileError::InvalidReference(format!(
                    "node {node:?} reference {:?} crosses a non-object value",
                    binding.reference
                )));
            };
            schema = properties.get(*part).ok_or_else(|| {
                WorkflowCompileError::InvalidReference(format!(
                    "node {node:?} references unknown field {:?}",
                    binding.reference
                ))
            })?;
        }
        return Ok(schema);
    }
    if parts.len() < 2 {
        return Err(WorkflowCompileError::InvalidReference(format!(
            "workflow reference {:?} must include a field path",
            binding.reference
        )));
    }
    let Some(mut schema) = available.0.get(parts[0]) else {
        return Err(WorkflowCompileError::InvalidReference(format!(
            "node {node:?} references unavailable producer {:?}",
            parts[0]
        )));
    };
    for part in &parts[1..] {
        let required = schema
            .get("required")
            .and_then(Value::as_array)
            .is_some_and(|required| required.iter().any(|value| value.as_str() == Some(part)));
        if !required {
            return Err(WorkflowCompileError::InvalidReference(format!(
                "node {node:?} reference {:?} crosses an optional field {:?}",
                binding.reference, part
            )));
        }
        let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
            return Err(WorkflowCompileError::InvalidReference(format!(
                "node {node:?} reference {:?} crosses a non-object value",
                binding.reference
            )));
        };
        schema = properties.get(*part).ok_or_else(|| {
            WorkflowCompileError::InvalidReference(format!(
                "node {node:?} references unknown field {:?}",
                binding.reference
            ))
        })?;
    }
    Ok(schema)
}

fn schema_type(schema: &Value) -> Option<&str> {
    schema.get("type").and_then(Value::as_str)
}

fn schema_types(schema: &Value) -> Option<BTreeSet<&str>> {
    match schema.get("type") {
        Some(Value::String(kind)) => Some(BTreeSet::from([kind.as_str()])),
        Some(Value::Array(kinds)) => {
            let kinds = kinds
                .iter()
                .map(Value::as_str)
                .collect::<Option<Vec<_>>>()?;
            Some(kinds.into_iter().collect())
        }
        _ => None,
    }
}

fn schema_required(schema: &Value) -> BTreeSet<&str> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

#[allow(clippy::too_many_lines)] // one bounded structural schema compatibility check
fn schemas_compatible(actual: &Value, expected: &Value) -> bool {
    if let Some(expected_types) = schema_types(expected) {
        let Some(actual_types) = schema_types(actual) else {
            return false;
        };
        if !actual_types.is_subset(&expected_types) {
            return false;
        }
    }

    if let Some(expected_const) = expected.get("const") {
        match (actual.get("const"), actual.get("enum")) {
            (Some(actual_const), _) => {
                if actual_const != expected_const {
                    return false;
                }
            }
            (_, Some(Value::Array(actual_enum))) => {
                if actual_enum.iter().any(|value| value != expected_const) {
                    return false;
                }
            }
            _ => return false,
        }
    }
    if let Some(Value::Array(expected_enum)) = expected.get("enum") {
        match (actual.get("const"), actual.get("enum")) {
            (Some(actual_const), _) => {
                if !expected_enum.iter().any(|value| value == actual_const) {
                    return false;
                }
            }
            (_, Some(Value::Array(actual_enum))) => {
                if actual_enum
                    .iter()
                    .any(|value| !expected_enum.iter().any(|expected| expected == value))
                {
                    return false;
                }
            }
            _ => return false,
        }
    }

    let expected_is_object = schema_types(expected).is_some_and(|types| types.contains("object"));
    let actual_is_object = schema_types(actual).is_some_and(|types| types.contains("object"));
    if expected_is_object && actual_is_object {
        let expected_properties = expected
            .get("properties")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let actual_properties = actual
            .get("properties")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let expected_required = schema_required(expected);
        let actual_required = schema_required(actual);
        for key in &expected_required {
            let Some(actual_property) = actual_properties.get(*key) else {
                return false;
            };
            let Some(expected_property) = expected_properties.get(*key) else {
                return false;
            };
            if !actual_required.contains(key) {
                return false;
            }
            if !schemas_compatible(actual_property, expected_property) {
                return false;
            }
        }
        for (key, expected_property) in &expected_properties {
            if let Some(actual_property) = actual_properties.get(key)
                && !schemas_compatible(actual_property, expected_property)
            {
                return false;
            }
        }
        if expected
            .get("additionalProperties")
            .is_some_and(|value| value == &Value::Bool(false))
            && (!actual
                .get("additionalProperties")
                .is_some_and(|value| value == &Value::Bool(false))
                || actual_properties
                    .keys()
                    .any(|key| !expected_properties.contains_key(key)))
        {
            return false;
        }
    }

    let expected_is_array = schema_types(expected).is_some_and(|types| types.contains("array"));
    let actual_is_array = schema_types(actual).is_some_and(|types| types.contains("array"));
    if expected_is_array && actual_is_array {
        match (actual.get("items"), expected.get("items")) {
            (Some(actual_items), Some(expected_items))
                if !schemas_compatible(actual_items, expected_items) =>
            {
                return false;
            }
            (None, Some(_)) => return false,
            _ => {}
        }
    }
    true
}

fn schemas_equivalent_enough(left: &Value, right: &Value) -> bool {
    schemas_compatible(left, right) && schemas_compatible(right, left)
}

/// The result of the reserved `workflow_output` terminal protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowOutputSubmission {
    /// The value passed the frozen Agent contract and was committed.
    Committed,
    /// The value was rejected without changing terminal state.
    Invalid(String),
    /// A valid output arrived after output or cancellation had already won.
    Stale,
}

/// The one terminal-output authority of a Workflow-owned `AgentRun`.
///
/// This is deliberately not a [`ToolExecutor`]. The model may see a
/// tool-shaped `workflow_output` declaration, but the call is consumed by
/// the Agent Loop before ordinary Tool Plane preflight or dispatch.
pub trait WorkflowOutputTerminal: Send + Sync {
    /// The frozen Agent output schema shown to the child model.
    fn output_schema(&self) -> Value;
    /// Attempts the validate-and-commit transition.
    fn submit(&self, value: Value) -> WorkflowOutputSubmission;
    /// Attempts the cancellation transition. A committed output cannot be
    /// rewritten by a later cancellation.
    fn cancel(&self, reason: crate::runtime::types::CancellationReason) -> bool;
}

#[derive(Debug)]
enum WorkflowOutputState {
    Pending,
    Committed(Value),
    Cancelled(crate::runtime::types::CancellationReason),
}

/// A thread-safe, frozen-schema terminal latch used by one child `AgentRun`.
pub struct WorkflowOutputLatch {
    schema: Value,
    validator: jsonschema::Validator,
    state: std::sync::Mutex<WorkflowOutputState>,
}

impl fmt::Debug for WorkflowOutputLatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkflowOutputLatch")
            .field("schema", &self.schema)
            .field(
                "state",
                &self
                    .state
                    .lock()
                    .map_or_else(|_| "poisoned".to_owned(), |state| format!("{state:?}")),
            )
            .finish_non_exhaustive()
    }
}

impl WorkflowOutputLatch {
    /// Creates a latch over one already compiler-validated Agent schema.
    ///
    /// # Errors
    ///
    /// Returns an error if the supplied schema cannot be compiled by the
    /// JSON Schema validator.
    pub fn new(schema: Value) -> Result<Self, String> {
        let validator = jsonschema::Validator::new(&schema)
            .map_err(|error| format!("invalid workflow Agent output schema: {error}"))?;
        Ok(Self {
            schema,
            validator,
            state: std::sync::Mutex::new(WorkflowOutputState::Pending),
        })
    }

    /// Returns the committed value, if output won the terminal race.
    ///
    /// # Panics
    ///
    /// Panics if the latch mutex is poisoned by a prior panic while holding
    /// it; a poisoned latch cannot safely establish terminal authority.
    #[must_use]
    pub fn committed_value(&self) -> Option<Value> {
        let state = self.state.lock().expect("workflow output latch lock");
        match &*state {
            WorkflowOutputState::Committed(value) => Some(value.clone()),
            WorkflowOutputState::Pending => None,
            WorkflowOutputState::Cancelled(reason) => {
                let _ = reason;
                None
            }
        }
    }
}

impl WorkflowOutputTerminal for WorkflowOutputLatch {
    fn output_schema(&self) -> Value {
        self.schema.clone()
    }

    fn submit(&self, value: Value) -> WorkflowOutputSubmission {
        let mut state = self.state.lock().expect("workflow output latch lock");
        if !matches!(*state, WorkflowOutputState::Pending) {
            return WorkflowOutputSubmission::Stale;
        }
        let serialized_size = match serde_json::to_vec(&value) {
            Ok(serialized) => serialized.len(),
            Err(_) => {
                return WorkflowOutputSubmission::Invalid(
                    "workflow_output must be JSON-serializable".to_owned(),
                );
            }
        };
        if serialized_size > crate::runtime::subagent::MAX_RESULT_CONTENT_BYTES {
            return WorkflowOutputSubmission::Invalid(
                "workflow_output exceeds the bounded value size".to_owned(),
            );
        }
        if !self.validator.is_valid(&value) {
            return WorkflowOutputSubmission::Invalid(
                "workflow_output does not satisfy the frozen Agent output schema".to_owned(),
            );
        }
        *state = WorkflowOutputState::Committed(value);
        WorkflowOutputSubmission::Committed
    }

    fn cancel(&self, reason: crate::runtime::types::CancellationReason) -> bool {
        let mut state = self.state.lock().expect("workflow output latch lock");
        if !matches!(*state, WorkflowOutputState::Pending) {
            return false;
        }
        *state = WorkflowOutputState::Cancelled(reason);
        true
    }
}

/// The terminal state of a dynamic workflow run.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkflowTerminalState {
    /// One validated workflow output was committed.
    Completed(Value),
    /// The workflow failed before producing a result.
    Failed(String),
    /// Cancellation won the workflow terminal race.
    Cancelled(crate::runtime::types::CancellationReason),
}

/// Dynamic execution state and ownership of one immutable program run.
///
/// A run owns only workflow-local values, deterministic control-flow
/// progression, admitted child identities, and terminal settlement. It does
/// not own model execution, tool execution, workspaces, or a second
/// scheduler.
pub struct WorkflowRun {
    program: Arc<WorkflowProgram>,
    run_id: ToolCallId,
    values: BTreeMap<String, Value>,
    active_children: BTreeSet<SubagentId>,
    terminal: Option<WorkflowTerminalState>,
}

impl fmt::Debug for WorkflowRun {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkflowRun")
            .field("program", &self.program.id())
            .field("run_id", &self.run_id)
            .field("values", &self.values.keys().collect::<Vec<_>>())
            .field("active_children", &self.active_children)
            .field("terminal", &self.terminal)
            .finish()
    }
}

impl WorkflowRun {
    fn new(program: Arc<WorkflowProgram>, run_id: ToolCallId) -> Self {
        Self {
            program,
            run_id,
            values: BTreeMap::new(),
            active_children: BTreeSet::new(),
            terminal: None,
        }
    }

    /// The immutable program snapshot this run owns.
    #[must_use]
    pub fn program(&self) -> &Arc<WorkflowProgram> {
        &self.program
    }

    /// The explicitly committed workflow-local values.
    #[must_use]
    pub fn values(&self) -> &BTreeMap<String, Value> {
        &self.values
    }

    /// The terminal settlement, once committed.
    #[must_use]
    pub fn terminal(&self) -> Option<&WorkflowTerminalState> {
        self.terminal.as_ref()
    }

    fn settle(&mut self, terminal: WorkflowTerminalState) -> Result<(), WorkflowRunError> {
        if self.terminal.is_some() {
            return Err(WorkflowRunError::TerminalAlreadySettled);
        }
        self.terminal = Some(terminal);
        Ok(())
    }
}

/// The native Workflow orchestrator over the existing `SubagentRegistry`.
#[derive(Clone)]
pub struct WorkflowRuntime {
    subagents: crate::runtime::subagent::SubagentRegistry,
    /// The existing conversation Event Journal. It records execution facts
    /// but never becomes the `WorkflowRun` state authority.
    event_store: Arc<dyn ConversationStore>,
}

impl fmt::Debug for WorkflowRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkflowRuntime")
            .field("subagents", &"native SubagentRegistry")
            .field("event_store", &"conversation Event Journal")
            .finish()
    }
}

/// Derives a stable, conversation-local identity for a Workflow fact.
///
/// Workflow events are observability evidence rather than execution
/// authority, but they still cross the durable Event Journal boundary. The
/// complete event payload includes the run and node identities, so hashing it
/// prevents distinct facts from colliding while keeping the event ID bounded.
fn workflow_event_id(event: &RuntimeEvent) -> EventId {
    let encoded = serde_json::to_vec(event).expect("Workflow runtime events are serializable");
    let digest = Sha256::digest(encoded);
    EventId::new(format!("workflow-event:{digest:x}"))
}

impl WorkflowRuntime {
    /// Creates the workflow orchestrator over one native child registry.
    #[must_use]
    pub fn new(
        subagents: crate::runtime::subagent::SubagentRegistry,
        event_store: Arc<dyn ConversationStore>,
    ) -> Self {
        Self {
            subagents,
            event_store,
        }
    }

    /// Executes one immutable foreground Workflow program.
    ///
    /// The returned value is suitable for the parent Workflow `ToolResult`.
    /// Intermediate values and child transcripts remain inside this run and
    /// never enter the parent canonical conversation.
    ///
    /// # Errors
    ///
    /// Returns a [`WorkflowRunError`] when input, child execution, control
    /// flow, output validation, or cancellation prevents successful
    /// settlement.
    pub async fn run_foreground(
        &self,
        program: Arc<WorkflowProgram>,
        run_id: ToolCallId,
        context: crate::runtime::subagent::AttemptSubagentContext,
        input: Value,
        cancellation: crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<Value, WorkflowRunError> {
        let mut run = WorkflowRun::new(program.clone(), run_id);
        self.emit(
            &run,
            RuntimeEvent::WorkflowStarted {
                workflow_id: program.id().clone(),
                run_id: run.run_id.clone(),
            },
        );
        let input_validator = jsonschema::Validator::new(program.input_schema())
            .map_err(|error| WorkflowRunError::InvalidInput(error.to_string()))?;
        if !input_validator.is_valid(&input) {
            return self.finish_failed(
                &mut run,
                WorkflowRunError::InvalidInput(
                    "workflow input does not satisfy the frozen input schema".to_owned(),
                ),
            );
        }
        let execution = self
            .execute_program(&mut run, &context, input, &cancellation)
            .await;
        match execution {
            Ok(value) => {
                let output_validator = jsonschema::Validator::new(program.output_schema())
                    .map_err(|error| WorkflowRunError::InvalidOutput(error.to_string()))?;
                if !output_validator.is_valid(&value) {
                    return self.finish_failed(
                        &mut run,
                        WorkflowRunError::InvalidOutput(
                            "workflow Return value does not satisfy the frozen output schema"
                                .to_owned(),
                        ),
                    );
                }
                // This synchronous check is the Workflow terminal
                // cancellation linearization point. Once it passes, the
                // value validation and `WorkflowRun::settle` below contain no
                // await, so a later cancellation cannot rewrite completion;
                // a cancellation observed here drains owned children before
                // publishing the Cancelled terminal.
                if cancellation.is_cancelled() {
                    return self.finish_cancelled(&mut run, &cancellation).await;
                }
                run.settle(WorkflowTerminalState::Completed(value.clone()))?;
                self.emit(
                    &run,
                    RuntimeEvent::WorkflowCompleted {
                        workflow_id: program.id().clone(),
                        run_id: run.run_id.clone(),
                    },
                );
                Ok(value)
            }
            Err(error) => {
                self.cancel_and_drain(&mut run, &cancellation).await;
                let terminal = match &error {
                    WorkflowRunError::Cancelled(reason) => {
                        WorkflowTerminalState::Cancelled(*reason)
                    }
                    _ => WorkflowTerminalState::Failed(error.to_string()),
                };
                run.settle(terminal)?;
                match &error {
                    WorkflowRunError::Cancelled(reason) => self.emit(
                        &run,
                        RuntimeEvent::WorkflowCancelled {
                            workflow_id: program.id().clone(),
                            run_id: run.run_id.clone(),
                            reason: *reason,
                        },
                    ),
                    _ => self.emit(
                        &run,
                        RuntimeEvent::WorkflowFailed {
                            workflow_id: program.id().clone(),
                            run_id: run.run_id.clone(),
                            diagnostic: bound_workflow_text(error.to_string()),
                        },
                    ),
                }
                Err(error)
            }
        }
    }

    fn finish_failed(
        &self,
        run: &mut WorkflowRun,
        error: WorkflowRunError,
    ) -> Result<Value, WorkflowRunError> {
        run.settle(WorkflowTerminalState::Failed(error.to_string()))?;
        self.emit(
            run,
            RuntimeEvent::WorkflowFailed {
                workflow_id: run.program.id().clone(),
                run_id: run.run_id.clone(),
                diagnostic: bound_workflow_text(error.to_string()),
            },
        );
        Err(error)
    }

    async fn finish_cancelled(
        &self,
        run: &mut WorkflowRun,
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<Value, WorkflowRunError> {
        let reason = cancellation.reason();
        self.cancel_and_drain(run, cancellation).await;
        let error = WorkflowRunError::Cancelled(reason);
        run.settle(WorkflowTerminalState::Cancelled(reason))?;
        self.emit(
            run,
            RuntimeEvent::WorkflowCancelled {
                workflow_id: run.program.id().clone(),
                run_id: run.run_id.clone(),
                reason,
            },
        );
        Err(error)
    }

    /// Appends one bounded observability fact to the conversation's existing
    /// Event Journal. The journal is intentionally best-effort here: it
    /// records Workflow facts but never decides control flow or terminal
    /// state, which remain owned by this `WorkflowRun` and its native child
    /// registry.
    fn emit(&self, _run: &WorkflowRun, event: RuntimeEvent) {
        let event_id = workflow_event_id(&event);
        let envelope = RuntimeEventEnvelope {
            schema_version: EVENT_SCHEMA_VERSION,
            event_id,
            sequence: 0,
            conversation_id: self.event_store.conversation_id().clone(),
            attempt_id: None,
            turn_id: None,
            timestamp: Utc::now(),
            event,
        };
        let _ = self.event_store.append_event(envelope);
    }

    async fn execute_program(
        &self,
        run: &mut WorkflowRun,
        context: &crate::runtime::subagent::AttemptSubagentContext,
        input: Value,
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<Value, WorkflowRunError> {
        let mut node_id = run.program.entry().to_owned();
        loop {
            if cancellation.is_cancelled() {
                return Err(WorkflowRunError::Cancelled(cancellation.reason()));
            }
            let node = run
                .program
                .nodes()
                .get(&node_id)
                .cloned()
                .ok_or_else(|| WorkflowRunError::InvalidProgram(node_id.clone()))?;
            match node {
                WorkflowNodeProgram::Agent(agent) => {
                    let value = self
                        .execute_agent(run, context, &input, &node_id, &agent, cancellation)
                        .await?;
                    run.values.insert(node_id.clone(), value);
                    node_id = single_successor(&run.program, &node_id)?;
                }
                WorkflowNodeProgram::Branch { condition } => {
                    let value = resolve_runtime_reference(&condition, &input, &run.values)?;
                    let Some(condition) = value.as_bool() else {
                        return Err(WorkflowRunError::InvalidValue(format!(
                            "Branch {node_id:?} condition did not produce a boolean"
                        )));
                    };
                    let port = if condition {
                        WorkflowPort::True
                    } else {
                        WorkflowPort::False
                    };
                    let successor = run
                        .program
                        .outgoing(&node_id)
                        .iter()
                        .find(|edge| edge.port == port)
                        .map(|edge| edge.to.clone())
                        .ok_or_else(|| WorkflowRunError::InvalidProgram(node_id.clone()))?;
                    self.emit(
                        run,
                        RuntimeEvent::WorkflowBranchSelected {
                            workflow_id: run.program.id().clone(),
                            run_id: run.run_id.clone(),
                            node_id: node_id.clone(),
                            port,
                            successor: successor.clone(),
                        },
                    );
                    node_id = successor;
                }
                WorkflowNodeProgram::Parallel { branches, .. } => {
                    let value = self
                        .execute_parallel(run, context, &input, &node_id, &branches, cancellation)
                        .await?;
                    run.values.insert(node_id.clone(), value);
                    node_id = single_successor(&run.program, &node_id)?;
                }
                WorkflowNodeProgram::Return { output } => {
                    let mut result = serde_json::Map::new();
                    for (key, binding) in output {
                        result.insert(
                            key,
                            resolve_runtime_reference(&binding, &input, &run.values)?.clone(),
                        );
                    }
                    return Ok(Value::Object(result));
                }
            }
        }
    }

    async fn execute_agent(
        &self,
        run: &mut WorkflowRun,
        context: &crate::runtime::subagent::AttemptSubagentContext,
        input: &Value,
        node_id: &str,
        agent: &WorkflowAgentProgram,
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<Value, WorkflowRunError> {
        let subagent_id = self
            .admit_agent(run, context, input, node_id, agent, cancellation)
            .await?;
        self.settle_agent(
            run,
            subagent_id,
            node_id,
            &agent.output_schema,
            cancellation,
        )
        .await
    }

    async fn admit_agent(
        &self,
        run: &mut WorkflowRun,
        context: &crate::runtime::subagent::AttemptSubagentContext,
        input: &Value,
        node_id: &str,
        agent: &WorkflowAgentProgram,
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<crate::runtime::identity::SubagentId, WorkflowRunError> {
        let resolved = context.resolve_workflow(&agent.profile).map_err(|error| {
            WorkflowRunError::ChildStart {
                node: node_id.to_owned(),
                detail: error.to_string(),
            }
        })?;
        if !resolved.model.primary.capabilities.tool_calls {
            return Err(WorkflowRunError::ChildStart {
                node: node_id.to_owned(),
                detail: "Workflow Agent profile resolves to a model without tool-call capability"
                    .to_owned(),
            });
        }
        let mut bound = serde_json::Map::new();
        for (key, binding) in &agent.input {
            bound.insert(
                key.clone(),
                resolve_runtime_reference(binding, input, &run.values)?.clone(),
            );
        }
        let context_package = serde_json::json!({
            "workflow_node": node_id,
            "input": Value::Object(bound),
        });
        let context_package = serde_json::to_string(&context_package).map_err(|error| {
            WorkflowRunError::ChildStart {
                node: node_id.to_owned(),
                detail: format!("cannot encode typed Agent input: {error}"),
            }
        })?;
        let spec = crate::runtime::subagent::SubagentStartSpec {
            resolved,
            approval_mode: context.approval_mode(),
            task: agent.task.clone(),
            context: Some(context_package),
            tool_call_id: crate::runtime::identity::ToolCallId::new(format!(
                "workflow:{}:{}:{}",
                run.program.id(),
                run.run_id,
                node_id
            )),
            terminal: crate::runtime::subagent::SubagentTerminalMode::WorkflowOutput {
                output_schema: agent.output_schema.clone(),
                workflow_id: run.program.id().clone(),
                run_id: run.run_id.clone(),
                node_id: node_id.to_owned(),
            },
        };
        let child_cancellation = cancellation.child_signal();
        let prepared = self
            .subagents
            .prepare(&spec, &child_cancellation)
            .await
            .map_err(|error| match error {
                crate::runtime::subagent::SubagentStartError::Cancelled => {
                    WorkflowRunError::Cancelled(cancellation.reason())
                }
                error => WorkflowRunError::ChildStart {
                    node: node_id.to_owned(),
                    detail: error.to_string(),
                },
            })?;
        let accepted = self
            .subagents
            .commit(prepared, &child_cancellation)
            .await
            .map_err(|error| match error {
                crate::runtime::subagent::SubagentStartError::Cancelled => {
                    WorkflowRunError::Cancelled(cancellation.reason())
                }
                error => WorkflowRunError::ChildStart {
                    node: node_id.to_owned(),
                    detail: error.to_string(),
                },
            })?;
        let crate::runtime::subagent::SubagentStartOutcome::Accepted(accepted) = accepted else {
            return Err(WorkflowRunError::Cancelled(cancellation.reason()));
        };
        run.active_children.insert(accepted.subagent_id.clone());
        self.emit(
            run,
            RuntimeEvent::WorkflowAgentAdmitted {
                workflow_id: run.program.id().clone(),
                run_id: run.run_id.clone(),
                node_id: node_id.to_owned(),
                subagent_id: accepted.subagent_id.clone(),
                profile: agent.profile.clone(),
            },
        );
        Ok(accepted.subagent_id)
    }

    async fn settle_agent(
        &self,
        run: &mut WorkflowRun,
        subagent_id: crate::runtime::identity::SubagentId,
        node_id: &str,
        output_schema: &Value,
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<Value, WorkflowRunError> {
        let mut wait = Box::pin(self.subagents.wait_until_settled(&subagent_id));
        let snapshot = tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                let _ = self.subagents.cancel(&subagent_id, cancellation.reason());
                let snapshot = (&mut wait).await;
                run.active_children.remove(&subagent_id);
                // The native child settlement is the cross-process
                // observation of the workflow_output latch. If that
                // success committed before cancellation, it remains the
                // winner even when this waiter observed cancellation first.
                if let Some(snapshot) = snapshot
                    && matches!(snapshot.state, crate::runtime::subagent::SubagentState::Succeeded)
                {
                    return Self::settled_agent_value(
                            snapshot,
                            node_id,
                            output_schema,
                            cancellation.reason(),
                        );
                }
                return Err(WorkflowRunError::Cancelled(cancellation.reason()));
            }
            snapshot = &mut wait => snapshot,
        };
        run.active_children.remove(&subagent_id);
        let snapshot = snapshot.ok_or_else(|| WorkflowRunError::ChildFailed {
            node: node_id.to_owned(),
            detail: "the native SubagentRegistry lost the child record".to_owned(),
        })?;
        Self::settled_agent_value(snapshot, node_id, output_schema, cancellation.reason())
    }

    fn settled_agent_value(
        snapshot: crate::runtime::subagent::SubagentSnapshot,
        node_id: &str,
        output_schema: &Value,
        cancellation_reason: crate::runtime::types::CancellationReason,
    ) -> Result<Value, WorkflowRunError> {
        match snapshot.state {
            crate::runtime::subagent::SubagentState::Succeeded => {
                let content = snapshot
                    .detail
                    .ok_or_else(|| WorkflowRunError::ChildFailed {
                        node: node_id.to_owned(),
                        detail: "workflow Agent completed without committed output".to_owned(),
                    })?;
                let value = serde_json::from_str(&content).map_err(|error| {
                    WorkflowRunError::ChildFailed {
                        node: node_id.to_owned(),
                        detail: format!("workflow Agent output was not JSON: {error}"),
                    }
                })?;
                let validator = jsonschema::Validator::new(output_schema).map_err(|error| {
                    WorkflowRunError::ChildFailed {
                        node: node_id.to_owned(),
                        detail: format!("workflow Agent output schema became invalid: {error}"),
                    }
                })?;
                if !validator.is_valid(&value) {
                    return Err(WorkflowRunError::ChildFailed {
                        node: node_id.to_owned(),
                        detail: "workflow Agent output violated its frozen output schema"
                            .to_owned(),
                    });
                }
                // The native SubagentRegistry already committed the value
                // fact atomically with the child's terminal lifecycle fact.
                // Do not append a second observability event here: the
                // WorkflowRun consumes that durable handoff but is not a
                // second Event Journal authority.
                Ok(value)
            }
            crate::runtime::subagent::SubagentState::Cancelled => {
                Err(WorkflowRunError::Cancelled(cancellation_reason))
            }
            state => Err(WorkflowRunError::ChildFailed {
                node: node_id.to_owned(),
                detail: snapshot
                    .detail
                    .unwrap_or_else(|| format!("native child settled as {state:?}")),
            }),
        }
    }

    async fn execute_parallel(
        &self,
        run: &mut WorkflowRun,
        context: &crate::runtime::subagent::AttemptSubagentContext,
        input: &Value,
        node_id: &str,
        branches: &BTreeMap<String, WorkflowAgentProgram>,
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<Value, WorkflowRunError> {
        let mut admitted = BTreeMap::new();
        let mut failures = BTreeMap::new();
        for (key, branch) in branches {
            if cancellation.is_cancelled() {
                return Err(WorkflowRunError::Cancelled(cancellation.reason()));
            }
            match self
                .admit_agent(
                    run,
                    context,
                    input,
                    &format!("{node_id}.{key}"),
                    branch,
                    cancellation,
                )
                .await
            {
                Ok(id) => {
                    admitted.insert(key.clone(), (id, branch));
                }
                Err(WorkflowRunError::Cancelled(reason)) => {
                    return Err(WorkflowRunError::Cancelled(reason));
                }
                Err(error) => {
                    failures.insert(key.clone(), error.to_string());
                }
            }
        }
        if !admitted.is_empty() {
            self.emit(
                run,
                RuntimeEvent::WorkflowParallelAdmitted {
                    workflow_id: run.program.id().clone(),
                    run_id: run.run_id.clone(),
                    node_id: node_id.to_owned(),
                    branches: admitted.keys().cloned().collect(),
                },
            );
        }
        let mut results = serde_json::Map::new();
        for (key, (subagent_id, branch)) in admitted {
            match self
                .settle_agent(
                    run,
                    subagent_id,
                    &format!("{node_id}.{key}"),
                    &branch.output_schema,
                    cancellation,
                )
                .await
            {
                Ok(value) => {
                    results.insert(key, value);
                }
                Err(WorkflowRunError::Cancelled(reason)) => {
                    return Err(WorkflowRunError::Cancelled(reason));
                }
                Err(error) => {
                    failures.insert(key, error.to_string());
                }
            }
        }
        self.emit(
            run,
            RuntimeEvent::WorkflowParallelSettled {
                workflow_id: run.program.id().clone(),
                run_id: run.run_id.clone(),
                node_id: node_id.to_owned(),
                succeeded: results.keys().cloned().collect(),
                failed: failures.keys().cloned().collect(),
            },
        );
        if !failures.is_empty() {
            let detail = failures
                .into_iter()
                .map(|(key, error)| format!("{key}: {error}"))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(WorkflowRunError::ParallelFailed {
                node: node_id.to_owned(),
                detail,
            });
        }
        Ok(Value::Object(results))
    }

    async fn cancel_and_drain(
        &self,
        run: &mut WorkflowRun,
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
    ) {
        let ids = run.active_children.iter().cloned().collect::<Vec<_>>();
        for id in &ids {
            let _ = self.subagents.cancel(id, cancellation.reason());
        }
        for id in ids {
            let _ = self.subagents.wait_until_settled(&id).await;
            run.active_children.remove(&id);
        }
    }
}

/// A workflow execution error. Execution failures remain failures; they are
/// never converted into workflow-local values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowRunError {
    /// Input did not satisfy the frozen workflow schema.
    InvalidInput(String),
    /// The Return value did not satisfy the frozen workflow schema.
    InvalidOutput(String),
    /// The immutable program was internally inconsistent.
    InvalidProgram(String),
    /// A committed reference could not be resolved at runtime.
    InvalidValue(String),
    /// A child could not be admitted.
    ChildStart { node: String, detail: String },
    /// A child settled unsuccessfully.
    ChildFailed { node: String, detail: String },
    /// One or more keyed parallel branches failed, in key order.
    ParallelFailed { node: String, detail: String },
    /// Cancellation won terminal settlement.
    Cancelled(crate::runtime::types::CancellationReason),
    /// A terminal transition was attempted twice.
    TerminalAlreadySettled,
}

impl WorkflowRunError {
    /// Whether this error represents the native cancellation terminal.
    #[must_use]
    pub const fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled(_))
    }
}

impl fmt::Display for WorkflowRunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(detail) => write!(formatter, "invalid workflow input: {detail}"),
            Self::InvalidOutput(detail) => write!(formatter, "invalid workflow output: {detail}"),
            Self::InvalidProgram(detail) => write!(formatter, "invalid workflow program: {detail}"),
            Self::InvalidValue(detail) => write!(formatter, "invalid workflow value: {detail}"),
            Self::ChildStart { node, detail } => {
                write!(
                    formatter,
                    "Workflow Agent {node:?} could not start: {detail}"
                )
            }
            Self::ChildFailed { node, detail } => {
                write!(formatter, "Workflow Agent {node:?} failed: {detail}")
            }
            Self::ParallelFailed { node, detail } => {
                write!(formatter, "Parallel {node:?} failed: {detail}")
            }
            Self::Cancelled(reason) => write!(formatter, "workflow cancelled: {reason:?}"),
            Self::TerminalAlreadySettled => {
                formatter.write_str("workflow terminal state was already settled")
            }
        }
    }
}

impl std::error::Error for WorkflowRunError {}

fn bound_workflow_text(value: String) -> String {
    crate::runtime::subagent::bound_utf8(value, crate::runtime::subagent::MAX_RESULT_CONTENT_BYTES)
}

fn single_successor(program: &WorkflowProgram, node: &str) -> Result<String, WorkflowRunError> {
    let edges = program.outgoing(node);
    if edges.len() != 1 || edges[0].port != WorkflowPort::Next {
        return Err(WorkflowRunError::InvalidProgram(format!(
            "node {node:?} does not have one Next successor"
        )));
    }
    Ok(edges[0].to.clone())
}

fn resolve_runtime_reference<'a>(
    binding: &WorkflowBinding,
    input: &'a Value,
    values: &'a BTreeMap<String, Value>,
) -> Result<&'a Value, WorkflowRunError> {
    let parts = binding.reference.split('.').collect::<Vec<_>>();
    if parts.len() < 2
        || parts.len() > MAX_REFERENCE_COMPONENTS
        || parts.iter().any(|part| part.is_empty())
    {
        return Err(WorkflowRunError::InvalidValue(format!(
            "malformed workflow reference {:?}",
            binding.reference
        )));
    }
    let mut value = if parts[0] == "args" {
        input
    } else {
        values.get(parts[0]).ok_or_else(|| {
            WorkflowRunError::InvalidValue(format!(
                "workflow producer {:?} is not committed",
                parts[0]
            ))
        })?
    };
    // Both input (`args.task`) and committed-node (`review.passed`)
    // references consume the root component while selecting their source.
    // Only the remaining path components are fields of that source value.
    for part in &parts[1..] {
        value = value.get(*part).ok_or_else(|| {
            WorkflowRunError::InvalidValue(format!(
                "workflow reference {:?} has no field {:?}",
                binding.reference, part
            ))
        })?;
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Barrier};
    use std::thread;

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

    fn agent(output: Value) -> WorkflowNodeDefinition {
        WorkflowNodeDefinition::Agent {
            profile: profile("reviewer"),
            task: "Review the input.".to_owned(),
            input: BTreeMap::from([(
                "task".to_owned(),
                WorkflowBinding {
                    reference: "args.task".to_owned(),
                },
            )]),
            output,
        }
    }

    fn return_node(output: BTreeMap<String, WorkflowBinding>) -> WorkflowNodeDefinition {
        WorkflowNodeDefinition::Return { output }
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
        output: Value,
    ) -> WorkflowDefinition {
        WorkflowDefinition {
            description: "Test workflow".to_owned(),
            input: schema(json!({"task": {"type": "string"}}), &["task"]),
            output,
            entry: entry.to_owned(),
            nodes,
            edges,
        }
    }

    fn compile_test(
        definition: WorkflowDefinition,
    ) -> Result<WorkflowProgram, WorkflowCompileError> {
        WorkflowProgram::compile(
            WorkflowId::parse("test_workflow").expect("id"),
            definition,
            &BTreeSet::from([profile("reviewer")]),
        )
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
            description: "Review".to_owned(),
            input: schema(json!({"task": {"type": "string"}}), &["task"]),
            output: schema(json!({"summary": {"type": "string"}}), &["summary"]),
            entry: "review".to_owned(),
            nodes: BTreeMap::from([
                ("review".to_owned(), agent(output)),
                (
                    "done".to_owned(),
                    WorkflowNodeDefinition::Return {
                        output: BTreeMap::from([(
                            "summary".to_owned(),
                            WorkflowBinding {
                                reference: "review.summary".to_owned(),
                            },
                        )]),
                    },
                ),
            ]),
            edges: vec![WorkflowEdgeDefinition {
                from: "review".to_owned(),
                to: "done".to_owned(),
                port: None,
            }],
        };
        let program = WorkflowProgram::compile(
            WorkflowId::parse("review_pr").expect("id"),
            definition,
            &BTreeSet::from([profile("reviewer")]),
        )
        .expect("program");
        assert_eq!(program.entry(), "review");
        assert_eq!(program.nodes().len(), 2);
    }

    #[test]
    fn rejects_branch_without_boolean_condition_or_complete_ports() {
        let definition = WorkflowDefinition {
            description: "Branch".to_owned(),
            input: schema(json!({"flag": {"type": "string"}}), &["flag"]),
            output: schema(json!({"value": {"type": "string"}}), &["value"]),
            entry: "decision".to_owned(),
            nodes: BTreeMap::from([(
                "decision".to_owned(),
                WorkflowNodeDefinition::Branch {
                    condition: WorkflowBinding {
                        reference: "args.flag".to_owned(),
                    },
                },
            )]),
            edges: Vec::new(),
        };
        let error = WorkflowProgram::compile(
            WorkflowId::parse("branch").expect("id"),
            definition,
            &BTreeSet::new(),
        )
        .expect_err("invalid branch");
        assert!(matches!(
            error,
            WorkflowCompileError::IncompatibleReference(_)
        ));
    }

    #[test]
    fn yaml_is_serialization_and_does_not_supply_identity() {
        let yaml = "description: Review\ninput: {type: object, properties: {task: {type: string}}, required: [task]}\noutput: {type: object, properties: {summary: {type: string}}, required: [summary]}\nentry: done\nnodes:\n  done:\n    type: return\n    output:\n      summary:\n        ref: args.task\nedges: []\n";
        let definition: WorkflowDefinition = serde_yaml::from_str(yaml).expect("yaml");
        assert!(WorkflowId::parse("review_pr").is_ok());
        assert!(!yaml.contains("name:"));
        assert_eq!(definition.entry, "done");
    }

    #[test]
    fn duplicate_yaml_node_ids_are_rejected_before_compilation() {
        let yaml = r"
description: Duplicate
input: {type: object}
output: {type: object}
entry: done
nodes:
  done: {type: return, output: {}}
  done: {type: return, output: {}}
edges: []
";
        assert!(serde_yaml::from_str::<WorkflowDefinition>(yaml).is_err());
    }

    #[test]
    fn rejects_dangling_unreachable_cyclic_and_unterminated_graphs() {
        let empty_output = schema(json!({}), &[]);
        let dangling = base_definition(
            "done",
            BTreeMap::from([("done".to_owned(), return_node(BTreeMap::new()))]),
            vec![edge("done", "missing")],
            empty_output.clone(),
        );
        assert!(matches!(
            compile_test(dangling),
            Err(WorkflowCompileError::DanglingReference(_))
        ));

        let unreachable = base_definition(
            "done",
            BTreeMap::from([
                ("done".to_owned(), return_node(BTreeMap::new())),
                ("orphan".to_owned(), return_node(BTreeMap::new())),
            ]),
            Vec::new(),
            empty_output.clone(),
        );
        assert!(matches!(
            compile_test(unreachable),
            Err(WorkflowCompileError::Unreachable(_))
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
            empty_output.clone(),
        );
        assert!(matches!(
            compile_test(cyclic),
            Err(WorkflowCompileError::Cycle)
        ));

        let unterminated = base_definition(
            "agent",
            BTreeMap::from([(
                "agent".to_owned(),
                agent(schema(json!({"ok": {"type": "boolean"}}), &["ok"])),
            )]),
            Vec::new(),
            empty_output,
        );
        assert!(matches!(
            compile_test(unterminated),
            Err(WorkflowCompileError::Unterminated(_))
        ));
    }

    #[test]
    fn rejects_branch_without_complete_ports() {
        let branch = WorkflowNodeDefinition::Branch {
            condition: WorkflowBinding {
                reference: "args.flag".to_owned(),
            },
        };
        let missing_ports = WorkflowDefinition {
            description: "Branch".to_owned(),
            input: schema(json!({"flag": {"type": "boolean"}}), &["flag"]),
            output: schema(json!({}), &[]),
            entry: "decision".to_owned(),
            nodes: BTreeMap::from([("decision".to_owned(), branch)]),
            edges: Vec::new(),
        };
        assert!(matches!(
            compile_test(missing_ports),
            Err(WorkflowCompileError::InvalidBranch(_))
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
                        profile: profile("reviewer"),
                        task: "Review the input.".to_owned(),
                        input: BTreeMap::from([(
                            "later".to_owned(),
                            WorkflowBinding {
                                reference: "later.summary".to_owned(),
                            },
                        )]),
                        output: output.clone(),
                    },
                ),
                ("done".to_owned(), return_node(BTreeMap::new())),
            ]),
            vec![edge("review", "done")],
            schema(json!({}), &[]),
        );
        assert!(matches!(
            compile_test(use_before),
            Err(WorkflowCompileError::InvalidReference(_))
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
                        WorkflowBinding {
                            reference: "review.summary".to_owned(),
                        },
                    )])),
                ),
            ]),
            vec![edge("review", "done")],
            schema(json!({"summary": {"type": "string"}}), &["summary"]),
        );
        assert!(matches!(
            compile_test(optional),
            Err(WorkflowCompileError::InvalidReference(_))
        ));
    }

    #[test]
    fn rejects_path_dependent_values_and_return_schema_mismatches() {
        let review_output = schema(json!({"passed": {"type": "boolean"}}), &["passed"]);
        let branch = WorkflowNodeDefinition::Branch {
            condition: WorkflowBinding {
                reference: "review.passed".to_owned(),
            },
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
                        profile: profile("reviewer"),
                        task: "Join the committed facts.".to_owned(),
                        input: BTreeMap::from([(
                            "summary".to_owned(),
                            WorkflowBinding {
                                reference: "yes.summary".to_owned(),
                            },
                        )]),
                        output: schema(json!({"ok": {"type": "boolean"}}), &["ok"]),
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
            schema(json!({}), &[]),
        );
        assert!(matches!(
            compile_test(path_dependent),
            Err(WorkflowCompileError::InvalidReference(_))
        ));

        let mismatch = base_definition(
            "review",
            BTreeMap::from([
                (
                    "review".to_owned(),
                    agent(schema(json!({"passed": {"type": "boolean"}}), &["passed"])),
                ),
                (
                    "done".to_owned(),
                    return_node(BTreeMap::from([(
                        "summary".to_owned(),
                        WorkflowBinding {
                            reference: "review.passed".to_owned(),
                        },
                    )])),
                ),
            ]),
            vec![edge("review", "done")],
            schema(json!({"summary": {"type": "string"}}), &["summary"]),
        );
        assert!(matches!(
            compile_test(mismatch),
            Err(WorkflowCompileError::IncompatibleReference(_))
        ));
    }

    #[test]
    fn rejects_nested_schema_mismatches_and_optional_nested_references() {
        let nested_boolean = schema(
            json!({
                "result": {
                    "type": "object",
                    "properties": {"passed": {"type": "boolean"}},
                    "required": ["passed"],
                    "additionalProperties": false
                }
            }),
            &["result"],
        );
        let nested_string_return = base_definition(
            "review",
            BTreeMap::from([
                ("review".to_owned(), agent(nested_boolean)),
                (
                    "done".to_owned(),
                    return_node(BTreeMap::from([(
                        "result".to_owned(),
                        WorkflowBinding {
                            reference: "review.result".to_owned(),
                        },
                    )])),
                ),
            ]),
            vec![edge("review", "done")],
            schema(
                json!({
                    "result": {
                        "type": "object",
                        "properties": {"passed": {"type": "string"}},
                        "required": ["passed"],
                        "additionalProperties": false
                    }
                }),
                &["result"],
            ),
        );
        assert!(matches!(
            compile_test(nested_string_return),
            Err(WorkflowCompileError::IncompatibleReference(_))
        ));

        let optional_input = WorkflowDefinition {
            description: "Optional nested input".to_owned(),
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
            output: schema(json!({}), &[]),
            entry: "review".to_owned(),
            nodes: BTreeMap::from([
                (
                    "review".to_owned(),
                    WorkflowNodeDefinition::Agent {
                        profile: profile("reviewer"),
                        task: "Review the input.".to_owned(),
                        input: BTreeMap::from([(
                            "detail".to_owned(),
                            WorkflowBinding {
                                reference: "args.task.detail".to_owned(),
                            },
                        )]),
                        output: schema(json!({}), &[]),
                    },
                ),
                ("done".to_owned(), return_node(BTreeMap::new())),
            ]),
            edges: vec![edge("review", "done")],
        };
        assert!(matches!(
            compile_test(optional_input),
            Err(WorkflowCompileError::InvalidReference(_))
        ));
    }

    #[test]
    fn parallel_keys_are_compiled_in_definition_order_and_tasks_are_static() {
        let branch_output = schema(json!({"summary": {"type": "string"}}), &["summary"]);
        let parallel = base_definition(
            "fanout",
            BTreeMap::from([
                (
                    "fanout".to_owned(),
                    WorkflowNodeDefinition::Parallel {
                        branches: BTreeMap::from([
                            (
                                "zulu".to_owned(),
                                WorkflowParallelBranchDefinition {
                                    profile: profile("reviewer"),
                                    task: "Review zulu.".to_owned(),
                                    input: BTreeMap::new(),
                                    output: branch_output.clone(),
                                },
                            ),
                            (
                                "alpha".to_owned(),
                                WorkflowParallelBranchDefinition {
                                    profile: profile("reviewer"),
                                    task: "Review alpha.".to_owned(),
                                    input: BTreeMap::new(),
                                    output: branch_output,
                                },
                            ),
                        ]),
                    },
                ),
                (
                    "done".to_owned(),
                    return_node(BTreeMap::from([(
                        "all".to_owned(),
                        WorkflowBinding {
                            reference: "fanout.alpha".to_owned(),
                        },
                    )])),
                ),
            ]),
            vec![edge("fanout", "done")],
            schema(json!({"all": {"type": "object"}}), &["all"]),
        );
        let program = compile_test(parallel).expect("parallel program");
        let WorkflowNodeProgram::Parallel { branches, .. } = program.nodes()["fanout"].clone()
        else {
            panic!("parallel node");
        };
        assert_eq!(
            branches.keys().cloned().collect::<Vec<_>>(),
            vec!["alpha", "zulu"]
        );

        let interpolated = base_definition(
            "review",
            BTreeMap::from([
                (
                    "review".to_owned(),
                    WorkflowNodeDefinition::Agent {
                        profile: profile("reviewer"),
                        task: "Review ${args.task}.".to_owned(),
                        input: BTreeMap::new(),
                        output: schema(json!({"ok": {"type": "boolean"}}), &["ok"]),
                    },
                ),
                ("done".to_owned(), return_node(BTreeMap::new())),
            ]),
            vec![edge("review", "done")],
            schema(json!({}), &[]),
        );
        assert!(matches!(
            compile_test(interpolated),
            Err(WorkflowCompileError::InvalidField(_))
        ));
    }

    #[test]
    fn workflow_catalog_rejects_unknown_and_duplicate_main_admission() {
        let definition = base_definition(
            "done",
            BTreeMap::from([("done".to_owned(), return_node(BTreeMap::new()))]),
            Vec::new(),
            schema(json!({}), &[]),
        );
        let program = compile_test(definition).expect("program");
        let unknown = WorkflowId::parse("missing").expect("id");
        assert!(matches!(
            WorkflowCatalog::new([program.clone()], [unknown]),
            Err(WorkflowCatalogError::UnknownMain(_))
        ));
        let id = program.id().clone();
        assert!(matches!(
            WorkflowCatalog::new([program.clone()], [id.clone(), id]),
            Err(WorkflowCatalogError::DuplicateMain(_))
        ));
        assert!(matches!(
            WorkflowCatalog::new([program.clone(), program], []),
            Err(WorkflowCatalogError::DuplicateDefinition)
        ));
    }

    #[test]
    fn runtime_reference_resolves_args_without_treating_args_as_a_field() {
        let input = json!({"task": "read this"});
        let binding = WorkflowBinding {
            reference: "args.task".to_owned(),
        };
        assert_eq!(
            resolve_runtime_reference(&binding, &input, &BTreeMap::new()).expect("reference"),
            &json!("read this")
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
                WorkflowOutputLatch::new(schema(
                    json!({"passed": {"type": "boolean"}}),
                    &["passed"],
                ))
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
        let run_id = ToolCallId::new("run-1");
        let started = RuntimeEvent::WorkflowStarted {
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
}
