//! Native, provider-independent scoped Workflow programs (Issues #83/#217).
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
#[cfg(test)]
use crate::runtime::identity::SubagentId;
use crate::runtime::identity::{EventId, ToolCallId};
pub use crate::tools::executor::WORKFLOW_OUTPUT_TOOL_NAME;

use super::subagent::SubagentName;

mod execution;
mod review;
pub use review::WorkflowReviewSubject;
mod expressions;
mod tool;
mod workspace;
use expressions::{
    evaluate_predicate, evaluate_value, valid_local_key, validate_predicate, value_schema,
};
pub use tool::WorkflowToolResult;
use tool::default_workflow_timeout_ms;

/// Runtime-owned identity, independent of model `ToolCall` text.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WorkflowRunId {
    /// Owning conversation.
    pub conversation_id: crate::runtime::identity::ConversationId,
    /// Native admitted attempt, unique across process recovery.
    pub attempt_id: crate::runtime::identity::AttemptId,
    /// WorkflowRuntime-owned invocation ordinal, allocated at run admission.
    pub invocation: u64,
}

/// Static source location; never a concrete execution authority.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WorkflowDefinitionPath {
    pub workflow_id: WorkflowId,
    /// Alternating Parallel node and branch keys; empty for root.
    pub blocks: Vec<String>,
}

/// A concrete block instance. Future iterations vary invocation components.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WorkflowBlockInstance {
    pub run: WorkflowRunId,
    pub definition: WorkflowDefinitionPath,
    pub invocations: Vec<u32>,
}

/// One concrete node visit in an owning block instance.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WorkflowNodeInstance {
    pub block: WorkflowBlockInstance,
    pub node: String,
    pub visit: u32,
}

/// Bounded block/node observation; live state stays in the executor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowExecutionOutcome {
    Completed,
    Failed,
    Cancelled,
    Denied,
    TimedOut,
    OutcomeUnknown,
}

impl fmt::Display for WorkflowNodeInstance {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}:{}:{}",
            format_args!(
                "{}:{}",
                self.block.run.attempt_id, self.block.run.invocation
            ),
            self.block.definition.blocks.join("."),
            self.node,
            self.visit
        )
    }
}

#[cfg(test)]
pub(crate) fn test_instance(workflow: &str, node: &str) -> WorkflowNodeInstance {
    WorkflowNodeInstance {
        block: WorkflowBlockInstance {
            run: WorkflowRunId {
                conversation_id: crate::runtime::identity::ConversationId::new(
                    "test-workflow-conversation",
                ),
                attempt_id: crate::runtime::identity::AttemptId::new("test-workflow-execution"),
                invocation: 1,
            },
            definition: WorkflowDefinitionPath {
                workflow_id: WorkflowId::parse(workflow).expect("test workflow"),
                blocks: Vec::new(),
            },
            invocations: vec![0],
        },
        node: node.into(),
        visit: 0,
    }
}

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
/// Aggregate nested block depth, with root at zero.
pub const MAX_BLOCK_DEPTH: usize = 8;
/// Maximum expression/schema/value nesting.
pub const MAX_VALUE_DEPTH: usize = 32;
/// Maximum serialized construction or value at a commit boundary.
pub const MAX_VALUE_BYTES: usize = 64 * 1024;
/// Aggregate retained inputs, locals and exported branch results in one run.
pub const MAX_LOCAL_BYTES: usize = 4 * 1024 * 1024;
/// Bound on each native run identity component; static keys have separate bounds.
pub const MAX_RUN_ID_COMPONENT_BYTES: usize = 256;
/// Retained text per native failure or branch diagnostic, excluding its key.
pub const MAX_WORKFLOW_DIAGNOSTIC_BYTES: usize = 1024;
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
        if value == WORKFLOW_OUTPUT_TOOL_NAME {
            return Err(WorkflowIdError::ReservedModelName);
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
    /// The identity is reserved for the Workflow Agent terminal protocol.
    ReservedModelName,
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
            Self::ReservedModelName => write!(
                formatter,
                "workflow id {WORKFLOW_OUTPUT_TOOL_NAME:?} is reserved for Workflow Agent terminalization"
            ),
        }
    }
}

impl std::error::Error for WorkflowIdError {}

/// The canonical YAML/domain representation of one workflow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDefinition {
    /// Explicit run-scoped candidate acquisition; absent means no Git resource.
    #[serde(default)]
    pub workspace: Option<WorkflowWorkspace>,
    /// The model-facing description of the workflow Tool.
    pub description: String,
    /// Explicit capability admission, independent of main model exposure.
    #[serde(default)]
    pub tools: BTreeSet<crate::capabilities::selection::ToolSelector>,
    /// Trusted finite total foreground lifetime, including descendant waits.
    #[serde(default = "default_workflow_timeout_ms")]
    pub timeout_ms: u64,
    /// The root lexical execution scope.
    pub block: WorkflowBlock,
}

/// A managed candidate always preserves isolation. Dirty-parent opt-out is
/// explicit and retains the native committed-baseline/overlay semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowWorkspace {
    #[serde(default = "workspace::strict_parent")]
    pub require_clean_parent: bool,
}

/// A fixed lexical graph. Root and Parallel branches have identical semantics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowBlock {
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
    Review {
        subject: WorkflowReviewSubject,
        context: Vec<WorkflowValue>,
    },

    /// One statically selected, explicitly admitted foreground capability.
    Tool {
        selector: crate::capabilities::selection::ToolSelector,
        arguments: WorkflowValue,
        result: WorkflowToolResult,
    },
    /// One execution of an admitted named Subagent profile.
    Agent {
        /// The native named profile to resolve at `AgentRun` admission.
        profile: SubagentName,
        /// The fixed task instruction for this `AgentRun`.
        task: String,
        /// Explicit input bindings from workflow-local values.
        #[serde(default)]
        input: BTreeMap<String, WorkflowValue>,
        /// The frozen `AgentRun` output contract.
        output: Value,
    },
    /// Deterministic selection from one committed boolean value.
    Branch {
        /// The sole boolean condition binding.
        condition: WorkflowPredicate,
    },
    /// A finite keyed set of private lexical blocks.
    Parallel {
        /// Branches are keyed by definition identity, not completion order.
        #[serde(deserialize_with = "deserialize_unique_map")]
        branches: BTreeMap<String, WorkflowBranch>,
    },
    /// Constructs a value and completes exactly this owning block.
    Return {
        /// The owning block's declared result.
        output: WorkflowValue,
    },
}

/// Closed, explicitly tagged value syntax; no evaluation of expression strings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowValue {
    /// First component is `args` or a local producer; remaining components are fields.
    Reference { path: Vec<String> },
    /// Literal JSON, including objects and arrays, never interpreted as syntax.
    Literal { value: Value },
    /// Construct an object atomically.
    Object {
        #[serde(deserialize_with = "deserialize_unique_map")]
        fields: BTreeMap<String, WorkflowValue>,
    },
    /// Construct an array atomically.
    Array { items: Vec<WorkflowValue> },
}

/// Typed predicates. Equality requires matching scalar types; no coercion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowPredicate {
    /// A boolean value.
    Boolean { value: WorkflowValue },
    /// Equal scalar operands of the same static type.
    Equal {
        left: WorkflowValue,
        right: WorkflowValue,
    },
    /// Unequal scalar operands of the same static type.
    NotEqual {
        left: WorkflowValue,
        right: WorkflowValue,
    },
    /// Boolean negation.
    Not { predicate: Box<WorkflowPredicate> },
    /// Conjunction of a nonempty fixed list.
    And { predicates: Vec<WorkflowPredicate> },
    /// Disjunction of a nonempty fixed list.
    Or { predicates: Vec<WorkflowPredicate> },
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

/// One explicit input projection and fixed private Parallel block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowBranch {
    /// Evaluated in the parent scope and checked against the child input schema.
    pub input: WorkflowValue,
    /// The child's private graph and contracts.
    pub block: WorkflowBlock,
}

/// A compiled, immutable executable workflow.
#[derive(Debug, Clone)]
pub struct WorkflowProgram {
    workspace: Option<WorkflowWorkspace>,
    id: WorkflowId,
    description: String,
    block: WorkflowBlockProgram,
    total_nodes: usize,
    retained_bound: usize,
    tools: BTreeSet<crate::capabilities::selection::ToolSelector>,
    timeout_ms: u64,
}

/// Immutable compiled graph shared by root and every nested branch.
#[derive(Debug, Clone)]
pub struct WorkflowBlockProgram {
    path: Vec<String>,
    input_schema: Value,
    output_schema: Value,
    entry: String,
    nodes: BTreeMap<String, WorkflowNodeProgram>,
    outgoing: BTreeMap<String, Vec<WorkflowEdgeProgram>>,
}

impl WorkflowProgram {
    /// The admitted total wall-clock budget. Every step consumes this budget.
    #[must_use]
    pub fn timeout_ms(&self) -> u64 {
        self.timeout_ms
    }
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
        &self.block.input_schema
    }

    /// The immutable output schema.
    #[must_use]
    pub fn output_schema(&self) -> &Value {
        &self.block.output_schema
    }

    /// The explicit entry node.
    #[must_use]
    pub fn entry(&self) -> &str {
        &self.block.entry
    }

    /// The compiled nodes in deterministic id order.
    #[must_use]
    pub fn nodes(&self) -> &BTreeMap<String, WorkflowNodeProgram> {
        &self.block.nodes
    }

    /// The compiled outgoing edges of a node.
    #[must_use]
    pub fn outgoing(&self, node: &str) -> &[WorkflowEdgeProgram] {
        self.block.outgoing.get(node).map_or(&[], Vec::as_slice)
    }
}

/// A compiled node whose references and schemas have been admitted.
#[derive(Debug, Clone)]
pub enum WorkflowNodeProgram {
    Review {
        subject: WorkflowReviewSubject,
        context: Vec<WorkflowValue>,
    },

    Tool {
        selector: crate::capabilities::selection::ToolSelector,
        arguments: WorkflowValue,
        result: WorkflowToolResult,
    },
    /// One admitted `AgentRun` template.
    Agent(WorkflowAgentProgram),
    /// One boolean Branch.
    Branch { condition: WorkflowPredicate },
    /// One keyed finite fan-out of compiled private blocks.
    Parallel {
        branches: BTreeMap<String, WorkflowBranchProgram>,
        output_schema: Value,
    },
    /// The terminal Return operation.
    Return { output: WorkflowValue },
}

/// A compiled `AgentRun` template.
#[derive(Debug, Clone)]
pub struct WorkflowAgentProgram {
    /// The admitted native profile.
    pub profile: SubagentName,
    /// The fixed task string.
    pub task: String,
    /// Explicit input bindings.
    pub input: BTreeMap<String, WorkflowValue>,
    /// Frozen output contract.
    pub output_schema: Value,
}

/// Compiled explicit child input projection and private block.
#[derive(Debug, Clone)]
pub struct WorkflowBranchProgram {
    input: WorkflowValue,
    block: WorkflowBlockProgram,
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
    if definition.timeout_ms == 0 || definition.timeout_ms > 86_400_000 {
        return Err(WorkflowCompileError::InvalidField(
            "timeout_ms must be 1..=86400000".into(),
        ));
    }
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
    if serde_json::to_vec(&definition).map_or(true, |bytes| bytes.len() > MAX_WORKFLOW_BYTES) {
        return Err(WorkflowCompileError::InvalidField(
            "aggregate program size exceeded".into(),
        ));
    }
    let mut total_nodes = 0;
    let block = compile_block(
        definition.block,
        workflow_profiles,
        &definition.tools,
        Vec::new(),
        &mut total_nodes,
    )?;
    let retained_bound = execution::static_retained_bound(&block);
    if retained_bound > MAX_LOCAL_BYTES {
        return Err(WorkflowCompileError::InvalidField(
            "aggregate retained-data reservation exceeds the run budget".into(),
        ));
    }
    Ok(WorkflowProgram {
        workspace: definition.workspace,
        id,
        description: definition.description,
        block,
        total_nodes,
        retained_bound,
        tools: definition.tools,
        timeout_ms: definition.timeout_ms,
    })
}

#[allow(clippy::too_many_lines)]
fn compile_block(
    definition: WorkflowBlock,
    workflow_profiles: &BTreeSet<SubagentName>,
    admitted_tools: &BTreeSet<crate::capabilities::selection::ToolSelector>,
    path: Vec<String>,
    total_nodes: &mut usize,
) -> Result<WorkflowBlockProgram, WorkflowCompileError> {
    *total_nodes += definition.nodes.len();
    if path.len() / 2 > MAX_BLOCK_DEPTH || *total_nodes > MAX_WORKFLOW_NODES {
        return Err(WorkflowCompileError::InvalidField(
            "aggregate block depth/node bound exceeded".into(),
        ));
    }
    validate_root_schema(&definition.input, "input")?;
    validate_root_schema(&definition.output, "output")?;
    if definition.nodes.is_empty() || definition.nodes.len() > MAX_WORKFLOW_NODES {
        return Err(WorkflowCompileError::InvalidField(format!(
            "block must contain between one and {MAX_WORKFLOW_NODES} nodes"
        )));
    }
    if definition.entry.trim().is_empty() {
        return Err(WorkflowCompileError::InvalidEntry(
            "workflow entry must be explicit and non-empty".to_owned(),
        ));
    }
    for node_id in definition.nodes.keys() {
        if node_id == "args" || !valid_local_key(node_id) {
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
            WorkflowNodeDefinition::Review { subject, context } => {
                if !matches!(subject.value(), WorkflowValue::Reference { .. }) {
                    return Err(WorkflowCompileError::InvalidField(
                        "Review subject must reference a committed value".into(),
                    ));
                }
                let schema = value_schema(subject.value(), &available_before, &node_id, 0)?;
                if matches!(subject, WorkflowReviewSubject::Plan { .. })
                    && schema_type(&schema) != Some("object")
                {
                    return Err(WorkflowCompileError::InvalidField(
                        "Review plan must be structured".into(),
                    ));
                }
                if context.len() > 8 {
                    return Err(WorkflowCompileError::InvalidField(
                        "Review context exceeds eight entries".into(),
                    ));
                }
                for value in context {
                    value_schema(value, &available_before, &node_id, 0)?;
                }
                available_after.insert(
                    node_id.clone(),
                    available_before.with_prefix(&node_id, &review::result_schema()),
                );
                WorkflowNodeProgram::Review {
                    subject: subject.clone(),
                    context: context.clone(),
                }
            }
            WorkflowNodeDefinition::Tool {
                selector,
                arguments,
                result,
            } => {
                if !admitted_tools.contains(selector) {
                    return Err(WorkflowCompileError::InvalidField(format!(
                        "Tool {node_id} selects unadmitted capability {selector}"
                    )));
                }
                let input = value_schema(arguments, &available_before, &node_id, 0)?;
                if schema_type(&input) != Some("object") {
                    return Err(WorkflowCompileError::InvalidField(
                        "Tool arguments must construct an object".into(),
                    ));
                }
                let output = result.schema();
                validate_workflow_schema(&output, "Tool result")?;
                available_after.insert(
                    node_id.clone(),
                    available_before.with_prefix(&node_id, &output),
                );
                WorkflowNodeProgram::Tool {
                    selector: selector.clone(),
                    arguments: arguments.clone(),
                    result: result.clone(),
                }
            }
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
                    available_before.with_prefix(&node_id, output),
                );
                WorkflowNodeProgram::Agent(agent)
            }
            WorkflowNodeDefinition::Branch { condition } => {
                validate_predicate(condition, &available_before, &node_id, 0)?;
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
                    if !valid_local_key(key) {
                        return Err(WorkflowCompileError::InvalidField(format!(
                            "Parallel {node_id:?} branch key {key:?} must be non-empty, at most 64 bytes, and contain no dots"
                        )));
                    }
                    let actual = value_schema(&branch.input, &available_before, &node_id, 0)?;
                    if !schemas_compatible(&actual, &branch.block.input) {
                        return Err(WorkflowCompileError::IncompatibleReference(format!(
                            "branch {key} input contract mismatch"
                        )));
                    }
                    let mut child_path = path.clone();
                    child_path.extend([node_id.clone(), key.clone()]);
                    let block = compile_block(
                        branch.block.clone(),
                        workflow_profiles,
                        admitted_tools,
                        child_path,
                        total_nodes,
                    )?;
                    output_properties.insert(key.clone(), block.output_schema.clone());
                    compiled_branches.insert(
                        key.clone(),
                        WorkflowBranchProgram {
                            input: branch.input.clone(),
                            block,
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
                    available_before.with_prefix(&node_id, &output_schema),
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
                let actual = value_schema(output, &available_before, &node_id, 0)?;
                if !schemas_compatible(&actual, &definition.output) {
                    return Err(WorkflowCompileError::IncompatibleReference(format!(
                        "Return {node_id} output contract mismatch"
                    )));
                }
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
    // A finite DAG whose only zero-outdegree nodes are Returns necessarily
    // terminates on every path. Do not enumerate exponentially many paths.
    Ok(WorkflowBlockProgram {
        path,
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
    validate_workflow_schema(schema, label)?;
    if schema_type(schema) != Some("object") {
        return Err(WorkflowCompileError::InvalidSchema(format!(
            "workflow {label} schema must have root type object"
        )));
    }
    jsonschema::Validator::new(schema).map_err(|error| {
        WorkflowCompileError::InvalidSchema(format!("workflow {label}: {error}"))
    })?;
    Ok(())
}

/// The deliberately closed JSON Schema vocabulary accepted by Workflow v1.
///
/// This is not the general JSON Schema language. The compiler's compatibility
/// proof is sound because every value-constraining keyword it admits is handled
/// recursively by [`schemas_compatible`]. A new keyword must be added here and
/// to that proof together, otherwise it is rejected as an unsupported schema.
const WORKFLOW_SCHEMA_KEYWORDS: &[&str] = &[
    "additionalProperties",
    "const",
    "enum",
    "items",
    "properties",
    "required",
    "type",
];

const WORKFLOW_SCHEMA_TYPES: &[&str] = &[
    "array", "boolean", "integer", "null", "number", "object", "string",
];

#[allow(clippy::too_many_lines)] // one recursive closed-vocabulary schema validator
fn validate_workflow_schema(schema: &Value, path: &str) -> Result<(), WorkflowCompileError> {
    validate_workflow_schema_at(schema, path, 0)
}

#[allow(clippy::too_many_lines)]
fn validate_workflow_schema_at(
    schema: &Value,
    path: &str,
    depth: usize,
) -> Result<(), WorkflowCompileError> {
    if depth > MAX_VALUE_DEPTH {
        return Err(WorkflowCompileError::InvalidSchema(
            "schema depth exceeded".into(),
        ));
    }
    let Some(object) = schema.as_object() else {
        return Err(WorkflowCompileError::InvalidSchema(format!(
            "workflow schema {path:?} must be an object"
        )));
    };
    if let Some(unsupported) = object
        .keys()
        .find(|keyword| !WORKFLOW_SCHEMA_KEYWORDS.contains(&keyword.as_str()))
    {
        return Err(WorkflowCompileError::InvalidSchema(format!(
            "workflow schema {path:?} uses unsupported keyword {unsupported:?}; Workflow v1 accepts only {}",
            WORKFLOW_SCHEMA_KEYWORDS.join(", ")
        )));
    }
    let Some(kind) = object.get("type").and_then(Value::as_str) else {
        return Err(WorkflowCompileError::InvalidSchema(format!(
            "workflow schema {path:?} must declare one string type"
        )));
    };
    if !WORKFLOW_SCHEMA_TYPES.contains(&kind) {
        return Err(WorkflowCompileError::InvalidSchema(format!(
            "workflow schema {path:?} has unsupported type {kind:?}"
        )));
    }
    // The current validator's numeric const path compares through f64.
    // Do not use that approximation as a static finite-value proof. Ordinary
    // integer/number schemas and numeric data remain supported.
    if matches!(kind, "integer" | "number")
        && (object.contains_key("const") || object.contains_key("enum"))
    {
        return Err(WorkflowCompileError::InvalidSchema(format!(
            "workflow schema {path:?}: numeric const/enum are outside the conservative static subset"
        )));
    }

    if let Some(enum_values) = object.get("enum") {
        let Some(enum_values) = enum_values.as_array() else {
            return Err(WorkflowCompileError::InvalidSchema(format!(
                "workflow schema {path:?} enum must be a non-empty array"
            )));
        };
        if enum_values.is_empty()
            || enum_values
                .iter()
                .enumerate()
                .any(|(index, value)| enum_values[..index].contains(value))
        {
            return Err(WorkflowCompileError::InvalidSchema(format!(
                "workflow schema {path:?} enum must contain unique values"
            )));
        }
    }
    if let (Some(constant), Some(Value::Array(enum_values))) =
        (object.get("const"), object.get("enum"))
        && !enum_values.contains(constant)
    {
        return Err(WorkflowCompileError::InvalidSchema(format!(
            "workflow schema {path:?} const must be included in enum"
        )));
    }
    for value in object.get("const").into_iter().chain(
        object
            .get("enum")
            .and_then(Value::as_array)
            .into_iter()
            .flatten(),
    ) {
        expressions::bounded_value_bytes(value).map_err(|error| {
            WorkflowCompileError::InvalidSchema(format!("schema {path:?} finite value: {error}"))
        })?;
    }

    match kind {
        "object" => {
            if let Some(properties) = object.get("properties") {
                let Some(properties) = properties.as_object() else {
                    return Err(WorkflowCompileError::InvalidSchema(format!(
                        "workflow schema {path:?} properties must be an object"
                    )));
                };
                for (name, property) in properties {
                    validate_workflow_schema_at(
                        property,
                        &format!("{path}.properties.{name}"),
                        depth + 1,
                    )?;
                }
            }
            if let Some(required) = object.get("required") {
                let Some(required) = required.as_array() else {
                    return Err(WorkflowCompileError::InvalidSchema(format!(
                        "workflow schema {path:?} required must be an array of property names"
                    )));
                };
                let properties = object
                    .get("properties")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                for (index, name) in required.iter().enumerate() {
                    let Some(name) = name.as_str() else {
                        return Err(WorkflowCompileError::InvalidSchema(format!(
                            "workflow schema {path}.required[{index}] must be a string"
                        )));
                    };
                    if !properties.contains_key(name) {
                        return Err(WorkflowCompileError::InvalidSchema(format!(
                            "workflow schema {path:?} required field {name:?} has no property schema"
                        )));
                    }
                    if required[..index]
                        .iter()
                        .any(|prior| prior.as_str() == Some(name))
                    {
                        return Err(WorkflowCompileError::InvalidSchema(format!(
                            "workflow schema {path:?} required contains duplicate field {name:?}"
                        )));
                    }
                }
            }
            if let Some(additional) = object.get("additionalProperties")
                && !additional.is_boolean()
            {
                return Err(WorkflowCompileError::InvalidSchema(format!(
                    "workflow schema {path:?} additionalProperties must be boolean"
                )));
            }
            if object.contains_key("items") {
                return Err(WorkflowCompileError::InvalidSchema(format!(
                    "workflow schema {path:?} items is only valid for array schemas"
                )));
            }
        }
        "array" => {
            if let Some(items) = object.get("items") {
                if !items.is_object() {
                    return Err(WorkflowCompileError::InvalidSchema(format!(
                        "workflow schema {path:?} items must be one schema object"
                    )));
                }
                validate_workflow_schema_at(items, &format!("{path}.items"), depth + 1)?;
            }
            for keyword in ["properties", "required", "additionalProperties"] {
                if object.contains_key(keyword) {
                    return Err(WorkflowCompileError::InvalidSchema(format!(
                        "workflow schema {path:?} {keyword} is only valid for object schemas"
                    )));
                }
            }
        }
        _ => {
            for keyword in ["properties", "required", "additionalProperties", "items"] {
                if object.contains_key(keyword) {
                    return Err(WorkflowCompileError::InvalidSchema(format!(
                        "workflow schema {path:?} {keyword} is incompatible with scalar type {kind:?}"
                    )));
                }
            }
        }
    }
    Ok(())
}

fn validate_agent(
    profile: &SubagentName,
    task: &str,
    input: &BTreeMap<String, WorkflowValue>,
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
        value_schema(binding, available, node, 0)?;
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

#[derive(Debug, Clone, Default)]
struct SchemaMap(BTreeMap<String, Value>);

impl SchemaMap {
    fn from_schema(schema: &Value) -> Self {
        Self(BTreeMap::from([("args".into(), schema.clone())]))
    }

    fn with_prefix(&self, prefix: &str, schema: &Value) -> Self {
        let mut result = self.clone();
        // Keep the complete frozen producer schema. Reducing it to required
        // fields would erase constraints such as enum/const and make a later
        // binding appear compatible merely because both values are objects.
        result.0.insert(prefix.to_owned(), schema.clone());
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

fn schema_type(schema: &Value) -> Option<&str> {
    schema.get("type").and_then(Value::as_str)
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

fn finite_schema_values(schema: &Value) -> Option<Vec<Value>> {
    if let Some(constant) = schema.get("const") {
        if let Some(Value::Array(enum_values)) = schema.get("enum") {
            return enum_values
                .contains(constant)
                .then(|| vec![constant.clone()]);
        }
        return Some(vec![constant.clone()]);
    }
    schema.get("enum").and_then(Value::as_array).cloned()
}

#[allow(clippy::too_many_lines)] // one bounded structural schema compatibility check
fn schemas_compatible(actual: &Value, expected: &Value) -> bool {
    // Finite producer schemas can be checked exactly against the complete
    // consumer schema. This is also the sound path for enum/const narrowing.
    if let Some(values) = finite_schema_values(actual) {
        if values.is_empty() {
            return false;
        }
        let Ok(expected_validator) = jsonschema::Validator::new(expected) else {
            return false;
        };
        return values
            .iter()
            .all(|value| expected_validator.is_valid(value));
    }
    if finite_schema_values(expected).is_some() {
        // An unrestricted producer is not statically known to satisfy a
        // finite consumer contract.
        return false;
    }

    let Some(actual_type) = schema_type(actual) else {
        return false;
    };
    let Some(expected_type) = schema_type(expected) else {
        return false;
    };
    if actual_type != expected_type && !(actual_type == "integer" && expected_type == "number") {
        return false;
    }

    if expected_type == "object" {
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
            if !actual_required.contains(key)
                || !schemas_compatible(actual_property, expected_property)
            {
                return false;
            }
        }
        for (key, expected_property) in &expected_properties {
            match actual_properties.get(key) {
                Some(actual_property)
                    if !schemas_compatible(actual_property, expected_property) =>
                {
                    return false;
                }
                None if !actual
                    .get("additionalProperties")
                    .is_some_and(|value| value == &Value::Bool(false)) =>
                {
                    // An undeclared producer property can still be emitted
                    // through additionalProperties and violate this consumer
                    // property's schema.
                    return false;
                }
                Some(_) | None => {}
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

    if expected_type == "array" {
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
    /// Workflow v1 compiler and JSON Schema validator.
    pub fn new(schema: Value) -> Result<Self, String> {
        validate_root_schema(&schema, "Workflow Agent output")
            .map_err(|error| error.to_string())?;
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
    Failed(WorkflowRunError),
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
    candidate: Option<crate::runtime::workspace::CandidateScope>,
    program: Arc<WorkflowProgram>,
    run_id: WorkflowRunId,
    budgets: std::sync::Mutex<execution::RunBudgets>,
    terminal: Option<WorkflowTerminalState>,
    tools:
        BTreeMap<crate::capabilities::selection::ToolSelector, crate::tools::types::ToolDefinition>,
}

impl fmt::Debug for WorkflowRun {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkflowRun")
            .field("candidate", &self.candidate)
            .field("program", &self.program.id())
            .field("run_id", &self.run_id)
            .field("budgets", &self.budgets)
            .field("terminal", &self.terminal)
            .field("tools", &self.tools)
            .finish()
    }
}

impl WorkflowRun {
    fn new(program: Arc<WorkflowProgram>, run_id: WorkflowRunId) -> Self {
        let budgets = execution::RunBudgets::reserved(program.retained_bound);
        Self {
            candidate: None,
            program,
            run_id,
            budgets: std::sync::Mutex::new(budgets),
            terminal: None,
            tools: BTreeMap::new(),
        }
    }

    /// The immutable program snapshot this run owns.
    #[must_use]
    pub fn program(&self) -> &Arc<WorkflowProgram> {
        &self.program
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
    /// The existing conversation Event Journal. Workflow lifecycle facts are
    /// best-effort observability here; the journal never becomes the
    /// `WorkflowRun` state authority.
    event_store: Arc<dyn ConversationStore>,
    next_run: Arc<std::sync::atomic::AtomicU64>,
    #[cfg(test)]
    node_frontier: Arc<std::sync::Mutex<Option<execution::NodeFrontierHook>>>,
    #[cfg(test)]
    observations: tokio::sync::watch::Sender<Vec<RuntimeEvent>>,
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
/// Workflow events are best-effort observability evidence rather than
/// execution authority. The complete event payload includes the run and node
/// identities, so hashing it prevents distinct facts from colliding while
/// keeping the event ID bounded.
fn workflow_event_id(event: &RuntimeEvent) -> EventId {
    match event {
        RuntimeEvent::WorkflowWorkspaceOwned { run_id, .. } => {
            return crate::runtime::workspace::workflow_resource_event_id(run_id, "owned");
        }
        RuntimeEvent::WorkflowWorkspaceSettled { run_id, .. } => {
            return crate::runtime::workspace::workflow_resource_event_id(run_id, "settled");
        }
        _ => {}
    }
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
            next_run: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            #[cfg(test)]
            node_frontier: Arc::new(std::sync::Mutex::new(None)),
            #[cfg(test)]
            observations: tokio::sync::watch::Sender::new(Vec::new()),
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
    #[allow(clippy::too_many_lines)] // One admission and terminal resource-settlement protocol.
    pub async fn run_foreground(
        &self,
        program: Arc<WorkflowProgram>,
        run_id: ToolCallId,
        context: crate::runtime::subagent::AttemptSubagentContext,
        input: Value,
        cancellation: crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<Value, WorkflowRunError> {
        // Candidate acquisition and final inspection are owned native work
        // too. Outer cancellation must drain this resource before starting
        // the composite settlement-control guard.
        let _workspace_owner = program.workspace.and_then(|_| {
            context
                .native
                .as_ref()
                .map(|native| native.descendants.enter())
        });
        if context.attempt_id().as_str().len() > MAX_RUN_ID_COMPONENT_BYTES
            || context.attempt_id().as_str().is_empty()
            || self.event_store.conversation_id().as_str().len() > MAX_RUN_ID_COMPONENT_BYTES
        {
            return Err(WorkflowRunError::InvalidInput(
                "native run identity exceeds its bound".into(),
            ));
        }
        let tool_call_id = run_id;
        let invocation = self
            .next_run
            .fetch_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |ordinal| ordinal.checked_add(1),
            )
            .map_err(|_| {
                WorkflowRunError::InvalidProgram("Workflow invocation identity exhausted".into())
            })?;
        let mut run = WorkflowRun::new(
            program.clone(),
            WorkflowRunId {
                conversation_id: self.event_store.conversation_id().clone(),
                attempt_id: context.attempt_id().clone(),
                invocation,
            },
        );
        self.emit_observability(
            &run,
            RuntimeEvent::WorkflowStarted {
                tool_call_id,
                workflow_id: program.id().clone(),
                run_id: run.run_id.clone(),
            },
        );
        let execution = match tool::freeze(&program, &context) {
            Ok(tools) => {
                run.tools = tools;
                let admission =
                    match execution::validate_commit(&program.block.input_schema, &input) {
                        Ok(()) => self.prepare_workspace(&run, &context, &cancellation).await,
                        Err(error) => Err(error),
                    };
                match admission {
                    Ok(candidate) => {
                        run.candidate = candidate;
                        self.execute_block(
                            &run,
                            &program.block,
                            &context,
                            input.into(),
                            &cancellation,
                        )
                        .await
                        .map(|output| output.value.value.clone())
                    }
                    Err(error) => Err(error),
                }
            }
            Err(error) => Err(error),
        };
        let workspace = match &run.candidate {
            Some(candidate) => Some(candidate.settle().await),
            None => None,
        };
        let candidate_reference = match &run.candidate {
            Some(candidate) => candidate.final_reference().await,
            None => None,
        };
        let recovery_guard = match &run.candidate {
            Some(candidate) => candidate.recovery_guard().await,
            None => None,
        };
        let execution = self.commit_workspace_settlement(
            &run,
            workspace.as_ref(),
            candidate_reference.as_ref(),
            recovery_guard.as_ref(),
            execution,
        );
        // Run terminal frontier. The shared block executor has already
        // validated its result and settled all owned native work. No await
        // separates this cancellation observation from the unique run commit.
        let execution = match execution {
            Ok(_) if cancellation.is_cancelled() => {
                Err(WorkflowRunError::from_cancellation(&cancellation))
            }
            result => result,
        };
        match execution {
            Ok(value) => {
                run.settle(WorkflowTerminalState::Completed(value.clone()))?;
                self.emit_observability(
                    &run,
                    RuntimeEvent::WorkflowCompleted {
                        workflow_id: program.id().clone(),
                        run_id: run.run_id.clone(),
                    },
                );
                Ok(match workspace {
                    Some(workspace) => {
                        serde_json::json!({"output": value, "workspace": workspace, "candidate": candidate_reference})
                    }
                    None => value,
                })
            }
            Err(error) => {
                let terminal = match &error {
                    WorkflowRunError::Cancelled(reason) => {
                        WorkflowTerminalState::Cancelled(*reason)
                    }
                    _ => WorkflowTerminalState::Failed(error.clone()),
                };
                run.settle(terminal)?;
                match &error {
                    WorkflowRunError::Cancelled(reason) => self.emit_observability(
                        &run,
                        RuntimeEvent::WorkflowCancelled {
                            workflow_id: program.id().clone(),
                            run_id: run.run_id.clone(),
                            reason: *reason,
                        },
                    ),
                    _ => self.emit_observability(
                        &run,
                        RuntimeEvent::WorkflowFailed {
                            workflow_id: program.id().clone(),
                            run_id: run.run_id.clone(),
                            diagnostic: bound_workflow_text(error.to_string()),
                            status: error.execution_status(),
                        },
                    ),
                }
                Err(match workspace {
                    Some(workspace) => WorkflowRunError::WorkspaceSettlement {
                        candidate: candidate_reference,
                        error: Box::new(error),
                        workspace: Box::new(workspace),
                    },
                    None => error,
                })
            }
        }
    }

    /// Appends one bounded best-effort observability fact to the conversation's
    /// existing Event Journal. A failure is intentionally ignored: ordinary
    /// Workflow lifecycle/join events never decide control flow or terminal
    /// state, which remain owned by this `WorkflowRun` and its native child
    /// registry. The successful child value and native terminal lifecycle fact
    /// use the separate durable compound transition in `SubagentRegistry`.
    fn emit_observability(&self, _run: &WorkflowRun, event: RuntimeEvent) {
        #[cfg(test)]
        self.observations
            .send_modify(|events| events.push(event.clone()));
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

    #[allow(clippy::too_many_arguments)] // the explicit child admission boundary
    #[allow(clippy::too_many_lines)] // Candidate admission precedes the single native child admission path.
    async fn admit_agent(
        &self,
        run: &WorkflowRun,
        context: &crate::runtime::subagent::AttemptSubagentContext,
        input: &expressions::CommittedValue,
        values: &BTreeMap<String, expressions::CommittedValue>,
        node_id: &WorkflowNodeInstance,
        agent: &WorkflowAgentProgram,
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
        admitted_access: Option<crate::runtime::workspace::WorkspaceAccess>,
    ) -> Result<crate::runtime::identity::SubagentId, WorkflowRunError> {
        let resolved = context.resolve_workflow(&agent.profile).map_err(|error| {
            WorkflowRunError::ChildStart {
                node: node_id.to_string(),
                detail: bound_workflow_diagnostic(error.to_string()),
            }
        })?;
        if !resolved.model.primary.capabilities.tool_calls {
            return Err(WorkflowRunError::ChildStart {
                node: node_id.to_string(),
                detail: "Workflow Agent profile resolves to a model without tool-call capability"
                    .to_owned(),
            });
        }
        let mut bound = serde_json::Map::new();
        let mut applicability = expressions::CommittedValue::from(Value::Null);
        for (key, binding) in &agent.input {
            let value = evaluate_value(binding, input, values)?;
            applicability.depend_on(&value)?;
            bound.insert(key.clone(), value.value);
        }
        if let Some(access) = &admitted_access {
            if applicability
                .candidate
                .as_ref()
                .is_some_and(|reference| reference != access.input())
            {
                return Err(WorkflowRunError::InvalidValue(
                    "Agent input differs from accepted candidate".into(),
                ));
            }
        } else {
            applicability.assert_current(run).await?;
        }
        let context_package = serde_json::json!({
            "workflow_node": node_id.node,
            "input": Value::Object(bound),
        });
        let context_package = serde_json::to_string(&context_package).map_err(|error| {
            WorkflowRunError::ChildStart {
                node: node_id.to_string(),
                detail: format!("cannot encode typed Agent input: {error}"),
            }
        })?;
        let spec = crate::runtime::subagent::SubagentStartSpec {
            resolved,
            approval_mode: context.approval_mode(),
            task: agent.task.clone(),
            context: Some(context_package),
            tool_call_id: crate::runtime::identity::ToolCallId::new(format!(
                "workflow-child:{:x}",
                Sha256::digest(serde_json::to_vec(node_id).expect("instance serialization"))
            )),
            terminal: crate::runtime::subagent::SubagentTerminalMode::WorkflowOutput {
                output_schema: agent.output_schema.clone(),
                workflow_id: run.program.id().clone(),
                run_id: run.run_id.clone(),
                node_id: Box::new(node_id.clone()),
            },
        };
        let child_cancellation = cancellation.child_signal();
        let access = if admitted_access.is_some() {
            admitted_access
        } else {
            match &run.candidate {
                Some(candidate) => Some(
                    candidate
                        .borrow(
                            node_id.clone(),
                            applicability.candidate.as_ref(),
                            &child_cancellation,
                        )
                        .await
                        .map_err(|error| {
                            if cancellation.is_cancelled() {
                                WorkflowRunError::from_cancellation(cancellation)
                            } else {
                                WorkflowRunError::InvocationAuthority(error)
                            }
                        })?,
                ),
                None => None,
            }
        };
        let prepared = self
            .subagents
            .prepare_in_workspace(&spec, &child_cancellation, access)
            .await
            .map_err(|error| match error {
                crate::runtime::subagent::SubagentStartError::Cancelled => {
                    WorkflowRunError::from_cancellation(cancellation)
                }
                error => WorkflowRunError::ChildStart {
                    node: node_id.to_string(),
                    detail: bound_workflow_diagnostic(error.to_string()),
                },
            })?;
        let accepted = self
            .subagents
            .commit_waiting(prepared, &child_cancellation)
            .await
            .map_err(|error| match error {
                crate::runtime::subagent::SubagentStartError::Cancelled => {
                    WorkflowRunError::from_cancellation(cancellation)
                }
                error => WorkflowRunError::ChildStart {
                    node: node_id.to_string(),
                    detail: bound_workflow_diagnostic(error.to_string()),
                },
            })?;
        let crate::runtime::subagent::SubagentStartOutcome::Accepted(accepted) = accepted else {
            return Err(WorkflowRunError::from_cancellation(cancellation));
        };
        self.emit_observability(
            run,
            RuntimeEvent::WorkflowAgentAdmitted {
                workflow_id: run.program.id().clone(),
                run_id: run.run_id.clone(),
                node_id: node_id.clone(),
                subagent_id: accepted.subagent_id.clone(),
                profile: agent.profile.clone(),
            },
        );
        Ok(accepted.subagent_id)
    }

    async fn settle_agent(
        &self,
        subagent_id: crate::runtime::identity::SubagentId,
        node_id: &WorkflowNodeInstance,
        output_schema: &Value,
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<expressions::CommittedValue, WorkflowRunError> {
        let mut wait = Box::pin(self.subagents.wait_until_settled(&subagent_id));
        let snapshot = tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                // The child owns a separate attempt. Parent deadline intent is
                // ParentCancelled there, never a fabricated user request; this
                // Workflow retains the exact deadline in its typed run outcome.
                let reason = match cancellation.native_cause() {
                    crate::tools::deadline::ToolCancellationCause::Attempt(reason) => reason,
                    crate::tools::deadline::ToolCancellationCause::Deadline(_) => crate::runtime::types::CancellationReason::ParentCancelled,
                };
                let _ = self.subagents.cancel(&subagent_id, reason);
                let snapshot = (&mut wait).await;
                // The native child settlement is the cross-process
                // observation of the workflow_output latch. If that
                // success committed before cancellation, it remains the
                // winner even when this waiter observed cancellation first.
                if let Some(snapshot) = snapshot
                    && matches!(snapshot.state, crate::runtime::subagent::SubagentState::Succeeded)
                {
                    return self.settled_agent_value(
                            snapshot,
                            &subagent_id,
                            node_id,
                            output_schema,
                            cancellation,
                        );
                }
                return Err(WorkflowRunError::from_cancellation(cancellation));
            }
            snapshot = &mut wait => snapshot,
        };
        let snapshot = snapshot.ok_or_else(|| WorkflowRunError::ChildFailed {
            node: node_id.to_string(),
            detail: "the native SubagentRegistry lost the child record".to_owned(),
        })?;
        self.settled_agent_value(snapshot, &subagent_id, node_id, output_schema, cancellation)
    }

    fn settled_agent_value(
        &self,
        snapshot: crate::runtime::subagent::SubagentSnapshot,
        subagent_id: &crate::runtime::identity::SubagentId,
        node_id: &WorkflowNodeInstance,
        output_schema: &Value,
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<expressions::CommittedValue, WorkflowRunError> {
        match snapshot.state {
            crate::runtime::subagent::SubagentState::Succeeded => {
                // The committed output value is the live Workflow result
                // channel owned by the registry (Issue #178): the
                // observation snapshot deliberately never carries it.
                let content = self
                    .subagents
                    .take_workflow_agent_output(subagent_id, node_id)
                    .ok_or_else(|| WorkflowRunError::ChildFailed {
                        node: node_id.to_string(),
                        detail: "workflow Agent completed without committed output".to_owned(),
                    })?;
                let value = content.value;
                // This validation is not redundant with
                // `validate_workflow_candidate` in the registry: the registry
                // revalidates the untrusted cross-process child wire frame
                // before its durable commit, while this validates the run's
                // result against the node's own frozen `output_schema`
                // authority at consumption time. The two checks guard
                // different trust boundaries; both stay.
                let validator = jsonschema::Validator::new(output_schema).map_err(|error| {
                    WorkflowRunError::ChildFailed {
                        node: node_id.to_string(),
                        detail: format!("workflow Agent output schema became invalid: {error}"),
                    }
                })?;
                if !validator.is_valid(&value) {
                    return Err(WorkflowRunError::ChildFailed {
                        node: node_id.to_string(),
                        detail: "workflow Agent output violated its frozen output schema"
                            .to_owned(),
                    });
                }
                // The native SubagentRegistry already committed the value
                // fact atomically with the child's terminal lifecycle fact.
                // Do not append a second observability event here: the
                // WorkflowRun consumes that durable handoff but is not a
                // second Event Journal authority.
                Ok(expressions::CommittedValue {
                    value,
                    candidate: content.candidate,
                })
            }
            crate::runtime::subagent::SubagentState::Cancelled => {
                Err(WorkflowRunError::from_cancellation(cancellation))
            }
            state => Err(WorkflowRunError::ChildFailed {
                node: node_id.to_string(),
                detail: bound_workflow_diagnostic(
                    snapshot
                        .detail
                        .unwrap_or_else(|| format!("native child settled as {state:?}")),
                ),
            }),
        }
    }
}

/// A workflow execution error. Execution failures remain failures; they are
/// never converted into workflow-local values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowRunError {
    WorkspaceSettlement {
        candidate: Option<crate::runtime::workspace::CandidateReference>,
        error: Box<WorkflowRunError>,
        workspace: Box<crate::runtime::workspace::WorkspaceSettlement>,
    },
    SourceUnavailable(String),
    InvalidSelector(String),
    CapabilityNotAdmitted(String),
    IneligibleCapability(String),
    IdentityChanged(String),
    InvocationAuthority(String),
    /// A native leaf settled without successful business output.
    ToolFailed {
        node: String,
        status: crate::tools::types::ToolExecutionStatus,
    },
    /// Input did not satisfy the frozen workflow schema.
    InvalidInput(String),
    /// The Return value did not satisfy the frozen workflow schema.
    InvalidOutput(String),
    /// The immutable program was internally inconsistent.
    InvalidProgram(String),
    /// A committed reference could not be resolved at runtime.
    InvalidValue(String),
    /// A child could not be admitted.
    ChildStart {
        node: String,
        detail: String,
    },
    /// A child settled unsuccessfully.
    ChildFailed {
        node: String,
        detail: String,
    },
    /// One or more keyed parallel branches failed, in key order.
    ParallelFailed {
        node: String,
        failures: BTreeMap<String, WorkflowRunError>,
    },
    /// Cancellation won terminal settlement.
    Cancelled(crate::runtime::types::CancellationReason),
    /// An ancestor native deadline interrupted this scope.
    Deadline(crate::tools::deadline::ToolDeadlineKind),
    /// A terminal transition was attempted twice.
    TerminalAlreadySettled,
}

impl WorkflowRunError {
    fn from_cancellation(
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
    ) -> Self {
        match cancellation.native_cause() {
            crate::tools::deadline::ToolCancellationCause::Attempt(reason) => {
                Self::Cancelled(reason)
            }
            crate::tools::deadline::ToolCancellationCause::Deadline(kind) => Self::Deadline(kind),
        }
    }
    /// Preserve native execution certainty at every composite boundary.
    #[must_use]
    pub fn execution_status(&self) -> crate::tools::types::ToolExecutionStatus {
        use crate::tools::types::{ToolCancellationPhase, ToolExecutionStatus as Status};
        match self {
            Self::WorkspaceSettlement {
                error, workspace, ..
            } => {
                if workspace.unresolved_reason()
                    == Some(crate::runtime::workspace::WorkspaceUnresolvedReason::NestedContainment)
                {
                    Status::OutcomeUnknown {
                        detail: "candidate ownership could not reach proven physical settlement"
                            .into(),
                    }
                } else {
                    error.execution_status()
                }
            }
            Self::ToolFailed { status, .. } => status.clone(),
            Self::Deadline(_) => Status::TimedOut,
            Self::Cancelled(reason) => Status::Cancelled {
                reason: *reason,
                phase: ToolCancellationPhase::DuringExecution,
            },
            Self::ParallelFailed { failures, .. } => {
                let statuses = failures
                    .values()
                    .map(Self::execution_status)
                    .collect::<Vec<_>>();
                statuses
                    .iter()
                    .find(|s| matches!(s, Status::OutcomeUnknown { .. }))
                    .or_else(|| statuses.first())
                    .cloned()
                    .unwrap_or_else(|| Status::Failed {
                        error: self.to_string(),
                    })
            }
            _ => Status::Failed {
                error: self.to_string(),
            },
        }
    }
    /// Whether this error represents the native cancellation terminal.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        matches!(
            self.execution_status(),
            crate::tools::types::ToolExecutionStatus::Cancelled { .. }
        )
    }
}

impl fmt::Display for WorkflowRunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WorkspaceSettlement { error, .. } => error.fmt(formatter),
            Self::ToolFailed { node, status } => {
                write!(formatter, "Workflow Tool {node:?}: {status:?}")
            }
            Self::SourceUnavailable(detail) => {
                write!(formatter, "capability source unavailable: {detail}")
            }
            Self::InvalidSelector(detail) => {
                write!(formatter, "invalid capability selector: {detail}")
            }
            Self::CapabilityNotAdmitted(detail) => {
                write!(formatter, "capability not admitted: {detail}")
            }
            Self::IneligibleCapability(detail) => {
                write!(formatter, "ineligible Workflow leaf: {detail}")
            }
            Self::IdentityChanged(detail) => {
                write!(formatter, "frozen capability identity changed: {detail}")
            }
            Self::InvocationAuthority(detail) => {
                write!(formatter, "native invocation authority failed: {detail}")
            }
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
            Self::ParallelFailed { node, failures } => {
                write!(formatter, "Parallel {node:?} failed: ")?;
                for (key, failure) in failures {
                    write!(formatter, "{key}: {failure}; ")?;
                }
                Ok(())
            }
            Self::Cancelled(reason) => write!(formatter, "workflow cancelled: {reason:?}"),
            Self::Deadline(kind) => write!(
                formatter,
                "workflow interrupted by ancestor {kind:?} deadline"
            ),
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

fn bound_workflow_diagnostic(value: String) -> String {
    crate::runtime::subagent::bound_utf8(value, MAX_WORKFLOW_DIAGNOSTIC_BYTES)
}

fn single_successor(
    program: &WorkflowBlockProgram,
    node: &str,
) -> Result<String, WorkflowRunError> {
    let edges = program.outgoing.get(node).map_or(&[][..], Vec::as_slice);
    if edges.len() != 1 || edges[0].port != WorkflowPort::Next {
        return Err(WorkflowRunError::InvalidProgram(format!(
            "node {node:?} does not have one Next successor"
        )));
    }
    Ok(edges[0].to.clone())
}

#[cfg(test)]
mod tests {
    mod scoped;
    mod tools;
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Barrier};
    use std::thread;

    #[cfg(unix)]
    use crate::capabilities::CapabilitySnapshot;
    #[cfg(unix)]
    use crate::context::SessionContextPolicy;
    #[cfg(unix)]
    use crate::durable::ConversationStore;
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
        SubagentDefinition, SubagentProjectInstructionPolicy, SubagentRegistry,
        SubagentRegistryConfig, SubagentSpawnPlan,
    };
    #[cfg(unix)]
    use crate::runtime::types::{ApprovalMode, CancellationReason, SystemClock};
    #[cfg(unix)]
    use crate::runtime::workspace::{WorkspaceManager, WorkspacePolicy};
    #[cfg(unix)]
    use crate::skills::SkillSnapshot;
    #[cfg(unix)]
    use crate::tools::environment::ToolEnvironment;
    #[cfg(unix)]
    use crate::tools::executor::ToolRegistry;
    #[cfg(unix)]
    use crate::tools::mcp::McpRuntimeLeaseAuthority;

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

    fn agent_branch(task: String, output: Value) -> WorkflowBranch {
        WorkflowBranch {
            input: WorkflowValue::Literal { value: json!({}) },
            block: WorkflowBlock {
                input: schema(json!({}), &[]),
                output: output.clone(),
                entry: "work".into(),
                nodes: BTreeMap::from([
                    (
                        "work".into(),
                        WorkflowNodeDefinition::Agent {
                            profile: profile("reviewer"),
                            task,
                            input: BTreeMap::new(),
                            output,
                        },
                    ),
                    (
                        "done".into(),
                        WorkflowNodeDefinition::Return {
                            output: reference("work"),
                        },
                    ),
                ]),
                edges: vec![edge("work", "done")],
            },
        }
    }

    fn agent(output: Value) -> WorkflowNodeDefinition {
        WorkflowNodeDefinition::Agent {
            profile: profile("reviewer"),
            task: "Review the input.".to_owned(),
            input: BTreeMap::from([("task".to_owned(), reference("args.task"))]),
            output,
        }
    }

    fn return_node(output: BTreeMap<String, WorkflowValue>) -> WorkflowNodeDefinition {
        WorkflowNodeDefinition::Return {
            output: WorkflowValue::Object { fields: output },
        }
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
            workspace: None,
            tools: std::collections::BTreeSet::default(),
            timeout_ms: 600_000,
            description: "Test workflow".to_owned(),
            block: WorkflowBlock {
                input: schema(json!({"task": {"type": "string"}}), &["task"]),
                output,
                entry: entry.to_owned(),
                nodes,
                edges,
            },
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

    #[cfg(unix)]
    const WORKFLOW_TEST_MODELS: &str = r#"{
      "providers": {
        "local": {
          "baseUrl": "http://127.0.0.1:9/v1",
          "apiKey": "test-only-secret",
          "models": [{
            "id": "model",
            "protocol": "openai_chat_completions",
            "contextWindow": 128000,
            "maxOutputTokens": 512,
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
    }"#;

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
        let conversation_id = ConversationId::new("workflow-test-conversation");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(conversation_id.clone())
                .expect("workflow store"),
        );
        let mailbox =
            crate::runtime::inbound::ConversationInboundMailbox::over_store(store.clone());
        let registry = SubagentRegistry::new(SubagentRegistryConfig {
            conversation_id: conversation_id.clone(),
            agent_id: AgentId::new("workflow-test-parent"),
            mailbox,
            clock: Arc::new(SystemClock),
            monotonic_clock: Arc::new(crate::runtime::ManualMonotonicClock::new()),
            spawn: SubagentSpawnPlan {
                program: std::path::PathBuf::from("/nonexistent/rustx"),
                runtime_root: runtime_root.clone(),
                model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
                tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(
                ),
                agent_status: crate::context::AgentStatusConfig::default(),
                context: SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
            },
            workspace: WorkspaceManager::new(&workspace, &runtime_root),
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
    ) -> crate::runtime::subagent::AttemptSubagentContext {
        workflow_test_context_for_generation(
            plane,
            1,
            "Workflow test instructions",
            WorkflowCatalog::empty(),
        )
    }

    #[cfg(unix)]
    fn workflow_test_context_for_generation(
        plane: &WorkflowTestPlane,
        revision: u64,
        instructions: &str,
        workflow_catalog: WorkflowCatalog,
    ) -> crate::runtime::subagent::AttemptSubagentContext {
        workflow_test_context_with_policy(
            plane,
            revision,
            instructions,
            workflow_catalog,
            WorkspacePolicy::SharedWorkspace,
        )
    }

    #[cfg(unix)]
    fn workflow_test_context_with_policy(
        plane: &WorkflowTestPlane,
        revision: u64,
        instructions: &str,
        workflow_catalog: WorkflowCatalog,
        workspace_policy: WorkspacePolicy,
    ) -> crate::runtime::subagent::AttemptSubagentContext {
        let model_catalog = ModelCatalog::from_jsonc_slice(WORKFLOW_TEST_MODELS.as_bytes())
            .expect("workflow test model catalog");
        let models = ModelBindingRegistry::new(
            model_catalog
                .resolve(&MapCredentialEnvironment::default())
                .expect("workflow test model resolution"),
        )
        .expect("workflow test model bindings");
        let model = ModelRef::parse("local/model").expect("workflow test model");
        let reviewer = profile("reviewer");
        let definition = SubagentDefinition::new(
            reviewer.clone(),
            "Workflow test reviewer".to_owned(),
            instructions.to_owned(),
            plane.dir.path().join("reviewer.md"),
            Some(model.clone()),
            None,
            Vec::new(),
            Vec::new(),
            SubagentProjectInstructionPolicy {
                inherit: false,
                files: Vec::new(),
            },
            workspace_policy,
        )
        .expect("workflow test subagent definition");
        let catalog = crate::runtime::subagent::SubagentCatalog::new([definition])
            .expect("workflow test subagent catalog");
        let capabilities = Arc::new(CapabilitySnapshot::new(
            plane.conversation_id.clone(),
            plane.dir.path().join("workspace"),
            CapabilityRevision::new(1),
            Arc::new(ToolRegistry::new()),
            Arc::new(crate::capabilities::AvailableToolCatalog::default()),
            Arc::new(SkillSnapshot::new(Vec::new())),
            None,
            None,
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
            .with_subagent_admissions(BTreeSet::new(), BTreeSet::from([reviewer]))
            .with_workflow_catalog(workflow_catalog),
        );
        crate::runtime::subagent::AttemptSubagentContext::new(
            crate::runtime::identity::AttemptId::new("workflow-test-attempt"),
            resources,
            SessionModelConfig::of(model),
            models,
            ApprovalMode::Policy,
        )
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
        let branch_output = schema(json!({"summary": {"type": "string"}}), &["summary"]);
        let mut branches = BTreeMap::new();
        let mut returned = BTreeMap::new();
        let mut output_properties = serde_json::Map::new();
        for key in keys {
            branches.insert(
                (*key).to_owned(),
                agent_branch(
                    format!("Run the {key} workflow branch."),
                    branch_output.clone(),
                ),
            );
            returned.insert((*key).to_owned(), reference(&(format!("fanout.{key}"))));
            output_properties.insert((*key).to_owned(), branch_output.clone());
        }
        let definition = base_definition(
            "fanout",
            BTreeMap::from([
                (
                    "fanout".to_owned(),
                    WorkflowNodeDefinition::Parallel { branches },
                ),
                ("done".to_owned(), return_node(returned)),
            ]),
            vec![edge("fanout", "done")],
            schema(Value::Object(output_properties), keys),
        );
        Arc::new(compile_test(definition).expect("parallel runtime program"))
    }

    #[cfg(unix)]
    fn snapshot_test_program(result_field: &str, description: &str) -> Arc<WorkflowProgram> {
        let branch_output = schema(json!({"summary": {"type": "string"}}), &["summary"]);
        let definition = WorkflowDefinition {
            workspace: None,
            tools: std::collections::BTreeSet::default(),
            timeout_ms: 600_000,
            description: description.to_owned(),
            block: WorkflowBlock {
                input: schema(json!({}), &[]),
                output: schema(
                    json!({result_field: branch_output.clone()}),
                    &[result_field],
                ),
                entry: "fanout".to_owned(),
                nodes: BTreeMap::from([
                    (
                        "fanout".to_owned(),
                        WorkflowNodeDefinition::Parallel {
                            branches: BTreeMap::from([(
                                "alpha".to_owned(),
                                agent_branch(
                                    format!("Run the {description} branch."),
                                    branch_output,
                                ),
                            )]),
                        },
                    ),
                    (
                        "done".to_owned(),
                        return_node(BTreeMap::from([(
                            result_field.to_owned(),
                            reference("fanout.alpha"),
                        )])),
                    ),
                ]),
                edges: vec![edge("fanout", "done")],
            },
        };
        Arc::new(
            WorkflowProgram::compile(
                WorkflowId::parse("snapshot_workflow").expect("snapshot workflow id"),
                definition,
                &BTreeSet::from([profile("reviewer")]),
            )
            .expect("snapshot workflow program"),
        )
    }

    #[cfg(unix)]
    fn return_only_program() -> Arc<WorkflowProgram> {
        let output = schema(json!({"value": {"type": "string"}}), &["value"]);
        Arc::new(
            WorkflowProgram::compile(
                WorkflowId::parse("event_journal_workflow").expect("event workflow id"),
                WorkflowDefinition {
                    workspace: None,
                    tools: std::collections::BTreeSet::default(),
                    timeout_ms: 600_000,
                    description: "Event journal test workflow".to_owned(),
                    block: WorkflowBlock {
                        input: output.clone(),
                        output: output.clone(),
                        entry: "done".to_owned(),
                        nodes: BTreeMap::from([(
                            "done".to_owned(),
                            return_node(BTreeMap::from([(
                                "value".to_owned(),
                                reference("args.value"),
                            )])),
                        )]),
                        edges: Vec::new(),
                    },
                },
                &BTreeSet::new(),
            )
            .expect("return-only workflow program"),
        )
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn parallel_runtime_keys_results_by_definition_when_completion_is_reversed() {
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
                .run_foreground(
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
        assert!(plane.registry.unsettled_snapshot().is_empty());
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
                .run_foreground(
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
        let WorkflowRunError::ParallelFailed { .. } = error else {
            panic!("expected keyed parallel failure");
        };
        assert!(
            detail.find("fanout.alpha").expect("alpha diagnostic")
                < detail.find("fanout.zulu").expect("zulu diagnostic")
        );
        assert!(plane.registry.unsettled_snapshot().is_empty());
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
                .run_foreground(
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
        assert!(plane.registry.unsettled_snapshot().is_empty());
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
        let context = workflow_test_context(&plane);
        let program = parallel_test_program(&["alpha", "zulu"]);
        let (signal, cancellation) = workflow_cancellation();
        let task = tokio::spawn(async move {
            runtime
                .run_foreground(
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
        alpha.cancel_after_delegate().await;
        zulu.cancel_after_delegate().await;

        let error = task
            .await
            .expect("workflow task")
            .expect_err("workflow cancellation");
        assert!(error.is_cancelled());
        let snapshots = plane.registry.all_snapshots();
        assert_eq!(snapshots.len(), 2);
        assert!(snapshots.iter().all(|snapshot| {
            snapshot.state == crate::runtime::subagent::SubagentState::Cancelled && snapshot.settled
        }));
        assert!(plane.registry.unsettled_snapshot().is_empty());
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
        let old_catalog =
            WorkflowCatalog::new([(*old_program).clone()], [old_program.id().clone()])
                .expect("old workflow catalog");
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

        let (_, old_cancellation) = workflow_cancellation();
        let old_task = tokio::spawn({
            let runtime = runtime.clone();
            async move {
                runtime
                    .run_foreground(
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

        // Construct the replacement immutable generation while the first run
        // is still parked at its native child. The run owns the old Arc and
        // cannot observe this future-invocation program.
        let new_program = snapshot_test_program("new", "new generation");
        let new_catalog =
            WorkflowCatalog::new([(*new_program).clone()], [new_program.id().clone()])
                .expect("new workflow catalog");
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
        );
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
                    .run_foreground(
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
        assert!(plane.registry.unsettled_snapshot().is_empty());
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
            .run_foreground(
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
    fn workflow_runtime(plane: &WorkflowTestPlane) -> WorkflowRuntime {
        WorkflowRuntime::new(plane.registry.clone(), plane.store.clone())
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
            workspace: None,
            tools: std::collections::BTreeSet::default(),
            timeout_ms: 600_000,
            description: "Review".to_owned(),
            block: WorkflowBlock {
                input: schema(json!({"task": {"type": "string"}}), &["task"]),
                output: schema(json!({"summary": {"type": "string"}}), &["summary"]),
                entry: "review".to_owned(),
                nodes: BTreeMap::from([
                    ("review".to_owned(), agent(output)),
                    (
                        "done".to_owned(),
                        WorkflowNodeDefinition::Return {
                            output: WorkflowValue::Object {
                                fields: BTreeMap::from([(
                                    "summary".to_owned(),
                                    reference("review.summary"),
                                )]),
                            },
                        },
                    ),
                ]),
                edges: vec![WorkflowEdgeDefinition {
                    from: "review".to_owned(),
                    to: "done".to_owned(),
                    port: None,
                }],
            },
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
    fn workflow_output_is_not_a_valid_workflow_identity() {
        assert_eq!(
            WorkflowId::parse(WORKFLOW_OUTPUT_TOOL_NAME),
            Err(WorkflowIdError::ReservedModelName)
        );
    }

    #[test]
    fn rejects_branch_without_boolean_condition_or_complete_ports() {
        let definition = WorkflowDefinition {
            workspace: None,
            tools: std::collections::BTreeSet::default(),
            timeout_ms: 600_000,
            description: "Branch".to_owned(),
            block: WorkflowBlock {
                input: schema(json!({"flag": {"type": "string"}}), &["flag"]),
                output: schema(json!({"value": {"type": "string"}}), &["value"]),
                entry: "decision".to_owned(),
                nodes: BTreeMap::from([(
                    "decision".to_owned(),
                    WorkflowNodeDefinition::Branch {
                        condition: WorkflowPredicate::Boolean {
                            value: reference("args.flag"),
                        },
                    },
                )]),
                edges: Vec::new(),
            },
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
        let yaml = "description: Review\nblock:\n  input: {type: object, properties: {task: {type: string}}, required: [task]}\n  output: {type: object, properties: {summary: {type: string}}, required: [summary]}\n  entry: done\n  nodes:\n    done:\n      type: return\n      output:\n        type: object\n        fields:\n          summary: {type: reference, path: [args, task]}\n  edges: []\n";
        let definition: WorkflowDefinition = serde_yaml::from_str(yaml).expect("yaml");
        assert!(WorkflowId::parse("review_pr").is_ok());
        assert!(!yaml.contains("name:"));
        assert_eq!(definition.block.entry, "done");
    }

    #[test]
    fn duplicate_yaml_node_ids_are_rejected_before_compilation() {
        let yaml = r"
description: Duplicate
block:
  input: {type: object}
  output: {type: object}
  entry: done
  nodes:
    done: {type: return, output: {type: literal, value: {}}}
    done: {type: return, output: {type: literal, value: {}}}
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
            condition: WorkflowPredicate::Boolean {
                value: reference("args.flag"),
            },
        };
        let missing_ports = WorkflowDefinition {
            workspace: None,
            tools: std::collections::BTreeSet::default(),
            timeout_ms: 600_000,
            description: "Branch".to_owned(),
            block: WorkflowBlock {
                input: schema(json!({"flag": {"type": "boolean"}}), &["flag"]),
                output: schema(json!({}), &[]),
                entry: "decision".to_owned(),
                nodes: BTreeMap::from([("decision".to_owned(), branch)]),
                edges: Vec::new(),
            },
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
                        input: BTreeMap::from([("later".to_owned(), reference("later.summary"))]),
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
                        reference("review.summary"),
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
            condition: WorkflowPredicate::Boolean {
                value: reference("review.passed"),
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
                        input: BTreeMap::from([("summary".to_owned(), reference("yes.summary"))]),
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
                        reference("review.passed"),
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
                        reference("review.result"),
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
            workspace: None,
            tools: std::collections::BTreeSet::default(),
            timeout_ms: 600_000,
            description: "Optional nested input".to_owned(),
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
                                reference("args.task.detail"),
                            )]),
                            output: schema(json!({}), &[]),
                        },
                    ),
                    ("done".to_owned(), return_node(BTreeMap::new())),
                ]),
                edges: vec![edge("review", "done")],
            },
        };
        assert!(matches!(
            compile_test(optional_input),
            Err(WorkflowCompileError::InvalidReference(_))
        ));
    }

    fn single_agent_return_definition(
        agent_output: Value,
        workflow_output: Value,
    ) -> WorkflowDefinition {
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
            workflow_output,
        )
    }

    #[test]
    fn workflow_v1_schema_language_rejects_unsupported_constraints_recursively() {
        let root = |nested: Value| schema(json!({"result": nested}), &["result"]);
        let unsupported = [
            ("minLength", json!({"type": "string", "minLength": 5})),
            ("minimum", json!({"type": "number", "minimum": 1})),
            (
                "minItems",
                json!({
                    "type": "array",
                    "items": {"type": "string"},
                    "minItems": 1
                }),
            ),
            (
                "oneOf",
                json!({
                    "type": "string",
                    "oneOf": [{"type": "string"}, {"type": "null"}]
                }),
            ),
            (
                "anyOf",
                json!({
                    "type": "object",
                    "properties": {
                        "nested": {
                            "type": "string",
                            "anyOf": [{"type": "string"}]
                        }
                    },
                    "required": ["nested"]
                }),
            ),
        ];
        for (keyword, nested) in unsupported {
            let error = compile_test(single_agent_return_definition(
                root(nested.clone()),
                root(json!({"type": "string"})),
            ))
            .expect_err("unsupported Workflow schema constraint");
            assert!(
                matches!(error, WorkflowCompileError::InvalidSchema(ref detail) if detail.contains(keyword)),
                "{keyword} should be named in the schema rejection: {error:?}"
            );
        }

        let mut input_constraint = single_agent_return_definition(
            root(json!({"type": "string"})),
            root(json!({"type": "string"})),
        );
        input_constraint.block.input = root(json!({"type": "string", "pattern": "x"}));
        assert!(matches!(
            compile_test(input_constraint),
            Err(WorkflowCompileError::InvalidSchema(detail)) if detail.contains("pattern")
        ));

        let output_constraint = single_agent_return_definition(
            root(json!({"type": "string"})),
            root(json!({"type": "string", "maximum": 2})),
        );
        assert!(matches!(
            compile_test(output_constraint),
            Err(WorkflowCompileError::InvalidSchema(detail)) if detail.contains("maximum")
        ));

        let branch_output = root(json!({"type": "string", "maxLength": 8}));
        let parallel = base_definition(
            "fanout",
            BTreeMap::from([
                (
                    "fanout".to_owned(),
                    WorkflowNodeDefinition::Parallel {
                        branches: BTreeMap::from([(
                            "alpha".to_owned(),
                            agent_branch("Review alpha.".to_owned(), branch_output),
                        )]),
                    },
                ),
                ("done".to_owned(), return_node(BTreeMap::new())),
            ]),
            vec![edge("fanout", "done")],
            schema(json!({}), &[]),
        );
        assert!(matches!(
            compile_test(parallel),
            Err(WorkflowCompileError::InvalidSchema(detail)) if detail.contains("maxLength")
        ));
        assert!(
            WorkflowOutputLatch::new(root(json!({
                "type": "string",
                "minLength": 1
            })))
            .is_err()
        );
    }

    #[test]
    fn workflow_schema_compilation_rejects_unsupported_narrowing_and_accepts_safe_subset_values() {
        let plain_string = single_agent_return_definition(
            schema(json!({"result": {"type": "string"}}), &["result"]),
            schema(
                json!({"result": {"type": "string", "minLength": 5}}),
                &["result"],
            ),
        );
        assert!(matches!(
            compile_test(plain_string),
            Err(WorkflowCompileError::InvalidSchema(detail)) if detail.contains("minLength")
        ));

        let plain_number = single_agent_return_definition(
            schema(json!({"result": {"type": "number"}}), &["result"]),
            schema(
                json!({"result": {"type": "number", "minimum": 1}}),
                &["result"],
            ),
        );
        assert!(matches!(
            compile_test(plain_number),
            Err(WorkflowCompileError::InvalidSchema(detail)) if detail.contains("minimum")
        ));

        let plain_array = single_agent_return_definition(
            schema(
                json!({
                    "result": {"type": "array", "items": {"type": "string"}}
                }),
                &["result"],
            ),
            schema(
                json!({
                    "result": {
                        "type": "array",
                        "items": {"type": "string"},
                        "minItems": 1
                    }
                }),
                &["result"],
            ),
        );
        assert!(matches!(
            compile_test(plain_array),
            Err(WorkflowCompileError::InvalidSchema(detail)) if detail.contains("minItems")
        ));

        let enum_producer = schema(
            json!({"result": {"type": "string", "enum": ["a", "b"]}}),
            &["result"],
        );
        let enum_consumer = schema(
            json!({"result": {"type": "string", "enum": ["a"]}}),
            &["result"],
        );
        assert!(matches!(
            compile_test(single_agent_return_definition(enum_producer, enum_consumer)),
            Err(WorkflowCompileError::IncompatibleReference(_))
        ));

        let const_producer = schema(
            json!({"result": {"type": "string", "const": "a"}}),
            &["result"],
        );
        let const_consumer = schema(
            json!({"result": {"type": "string", "const": "b"}}),
            &["result"],
        );
        assert!(matches!(
            compile_test(single_agent_return_definition(
                const_producer,
                const_consumer
            )),
            Err(WorkflowCompileError::IncompatibleReference(_))
        ));

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
        let safe = single_agent_return_definition(
            schema(json!({"result": nested.clone()}), &["result"]),
            schema(json!({"result": nested}), &["result"]),
        );
        compile_test(safe).expect("supported object/array/enum/const schemas are compatible");
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
                                agent_branch("Review zulu.".to_owned(), branch_output.clone()),
                            ),
                            (
                                "alpha".to_owned(),
                                agent_branch("Review alpha.".to_owned(), branch_output),
                            ),
                        ]),
                    },
                ),
                (
                    "done".to_owned(),
                    return_node(BTreeMap::from([(
                        "all".to_owned(),
                        reference("fanout.alpha"),
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
        let binding = reference("args.task");
        assert_eq!(
            evaluate_value(&binding, &input.into(), &BTreeMap::new())
                .expect("reference")
                .value,
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
        let run_id = test_instance("review", "done").block.run;
        let started = RuntimeEvent::WorkflowStarted {
            tool_call_id: ToolCallId::new("run-1"),
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
