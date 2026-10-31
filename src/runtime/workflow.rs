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

mod admission;
pub mod authoring;
pub use authoring::{WorkflowModelPolicy, WorkflowToolSelector};
mod execution;
mod flow;
pub mod inspection;
pub mod read_model;
mod review;
pub mod scheduled;
pub use review::{MAX_REVIEW_ACTIONS, REVIEW_TIMEOUT_PORT, WorkflowReviewAction};
mod expressions;
pub(crate) mod model;
mod tool;
use expressions::{
    evaluate_predicate, evaluate_value, valid_local_key, validate_predicate, value_schema,
};
pub(crate) use tool::default_workflow_timeout_ms;
pub use tool::{WorkflowToolChoice, WorkflowToolResult, selectable_builtin_tools};

/// Runtime-owned identity, independent of model `ToolCall` text.
/// Retained with the canonical outer result, not with an execution owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct WorkflowToolIdentity {
    pub workflow_id: WorkflowId,
    pub program_digest: String,
}

/// A real native execution origin. Scheduled work has no Human Attempt.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowRunOrigin {
    Tool {
        attempt_id: crate::runtime::identity::AttemptId,
    },
    Scheduled {
        start_id: scheduled::lifecycle::ScheduledWorkflowStartId,
    },
}
impl WorkflowRunOrigin {
    #[must_use]
    pub fn attempt_id(&self) -> Option<&crate::runtime::identity::AttemptId> {
        match self {
            Self::Tool { attempt_id } => Some(attempt_id),
            Self::Scheduled { .. } => None,
        }
    }
}
impl From<crate::runtime::identity::AttemptId> for WorkflowRunOrigin {
    fn from(attempt_id: crate::runtime::identity::AttemptId) -> Self {
        Self::Tool { attempt_id }
    }
}
impl std::fmt::Display for WorkflowRunOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tool { attempt_id } => write!(f, "tool:{attempt_id}"),
            Self::Scheduled { start_id } => write!(f, "scheduled:{start_id}"),
        }
    }
}

/// Runtime-owned identity, independent of model `ToolCall` text.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct WorkflowRunId {
    /// Owning conversation.
    pub conversation_id: crate::runtime::identity::ConversationId,
    /// Real Tool or durable Scheduled origin, unique across recovery.
    pub origin: WorkflowRunOrigin,
    /// WorkflowRuntime-owned invocation ordinal, allocated at run admission.
    pub invocation: u64,
}

/// Static source location; never a concrete execution authority.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct WorkflowDefinitionPath {
    pub workflow_id: WorkflowId,
    /// Pairs of Loop owner node and the fixed child key `body`; empty for root.
    pub blocks: Vec<String>,
}

/// A concrete block instance. Root starts at zero; each Loop appends its one-based iteration.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct WorkflowBlockInstance {
    pub run: WorkflowRunId,
    pub definition: WorkflowDefinitionPath,
    pub invocations: Vec<u32>,
}

/// One concrete node visit in an owning block instance.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct WorkflowNodeInstance {
    pub block: WorkflowBlockInstance,
    pub node: String,
    pub visit: u32,
}

/// Bounded block/node observation; live state stays in the executor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(schemars::JsonSchema)]
pub enum WorkflowExecutionOutcome {
    Completed,
    Failed,
    Cancelled,
    Denied,
    TimedOut,
    OutcomeUnknown,
}

/// Why a Loop finished, as Dify's `completed_reason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(schemars::JsonSchema)]
pub enum WorkflowLoopExit {
    /// The break condition held or an Exit Loop node ran.
    LoopBreak,
    /// It ran `loop_count` rounds.
    LoopCompleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowLimit {
    Steps,
    Agents,
    ModelCalls,
    RetainedData,
}

impl fmt::Display for WorkflowNodeInstance {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}:{:?}:{}:{}",
            format_args!("{}:{}", self.block.run.origin, self.block.run.invocation),
            self.block.definition.blocks.join("."),
            self.block.invocations,
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
                    "conv_47f0e826-cb00-7b0b-8ce4-af52ffdacf51",
                ),
                origin: (crate::runtime::identity::AttemptId::new("test-workflow-execution"))
                    .into(),
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

/// A root block as a Loop or Iteration body: no input or output, no Return,
/// and its Start renamed `loop_start` so it hides no root node.
#[cfg(test)]
pub(crate) fn test_body(block: &Value) -> Value {
    let rename = |id: &str, start: &str| {
        if id == start {
            "loop_start".to_owned()
        } else {
            id.to_owned()
        }
    };
    let nodes = block["nodes"].as_object().expect("nodes");
    let start = nodes
        .iter()
        .find(|(_, node)| node["type"] == "start")
        .expect("Start")
        .0
        .clone();
    let kept = nodes
        .iter()
        .filter(|(_, node)| node["type"] != "return")
        .map(|(id, node)| (rename(id, &start), node.clone()))
        .collect::<serde_json::Map<_, _>>();
    let edges = block["edges"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|edge| kept.contains_key(&rename(edge["to"].as_str().unwrap_or_default(), &start)))
        .map(|edge| {
            let mut edge = edge.clone();
            edge["from"] = rename(edge["from"].as_str().unwrap_or_default(), &start).into();
            edge
        })
        .collect::<Vec<_>>();
    serde_json::json!({"nodes": kept, "edges": edges})
}

/// The maximum serialized executable definition, excluding file presentation.
pub const MAX_WORKFLOW_BYTES: usize = 512 * 1024;
/// The maximum number of nodes in one workflow program.
pub const MAX_WORKFLOW_NODES: usize = 256;
/// Admitted node and iteration steps across the entire run.
pub const MAX_WORKFLOW_STEPS: usize = 4096;
/// Native `AgentRuns` across the entire run, never replenished by a block.
pub const MAX_WORKFLOW_AGENTS: usize = 256;
/// Logical Auxiliary Model Calls across the entire run, separate from Agents.
pub const MAX_WORKFLOW_MODEL_CALLS: usize = 256;
/// Prompt messages accepted by a Model node, the system message included.
pub const MAX_MODEL_MESSAGES: usize = 32;
/// Fixed text bytes accepted in one Model prompt message.
pub const MAX_MODEL_MESSAGE_BYTES: usize = 32 * 1024;
/// Text and reference parts accepted in one Model prompt message.
pub const MAX_MODEL_MESSAGE_PARTS: usize = 64;
/// Dify's `LOOP_NODE_MAX_COUNT`: the most rounds one Loop may run.
pub const LOOP_NODE_MAX_COUNT: u32 = 100;
/// Dify's `MAX_PARALLEL_LIMIT`: the most Iteration rounds run at once.
pub const MAX_PARALLEL_LIMIT: u32 = 10;
/// The maximum number of discovered workflow definitions in one generation.
pub const MAX_WORKFLOW_DEFINITIONS: usize = 64;
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
#[derive(schemars::JsonSchema)]
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
#[derive(schemars::JsonSchema)]
pub struct WorkflowDefinition {
    /// The model-facing description of the workflow Tool.
    pub description: String,
    /// Trusted finite total foreground lifetime, including descendant waits.
    #[serde(default = "default_workflow_timeout_ms")]
    pub timeout_ms: u64,
    /// The root lexical execution scope.
    pub block: WorkflowBlock,
}

/// A fixed graph: the root, or a Loop or Iteration body. A body declares no
/// input or output: as in Dify, it reads the variables of the nodes around
/// it, its container's variables and its own nodes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct WorkflowBlock {
    /// The workflow input JSON Schema.
    #[serde(
        default = "empty_object_schema",
        skip_serializing_if = "is_default_schema"
    )]
    pub input: Value,
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
#[derive(schemars::JsonSchema)]
pub enum WorkflowNodeDefinition {
    /// The block's real Start node. Its input is the block input contract.
    Start {},
    /// One tool-free logical Auxiliary Call using the invoking Attempt's authority.
    Model {
        #[serde(default)]
        model: WorkflowModelPolicy,
        /// The prompt: a system message, then user and assistant turns.
        messages: Vec<WorkflowModelMessage>,
        output: WorkflowModelOutput,
        /// Retry a failed call before its failure is handled.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry: Option<WorkflowRetry>,
        /// What a call that still failed does.
        #[serde(default, skip_serializing_if = "WorkflowModelErrorStrategy::is_stop")]
        on_error: WorkflowModelErrorStrategy,
    },
    /// As Dify's Loop: run the body again and again, until its break
    /// condition holds after a round, an Exit Loop node ran, or `loop_count`
    /// rounds are done. Its value is its loop variables.
    Loop {
        /// Variables the body reads and an Assigner node writes, under the
        /// Loop's own ID.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        loop_variables: Vec<WorkflowLoopVariable>,
        /// Checked before the first round and after every round.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        break_condition: Option<WorkflowPredicate>,
        loop_count: u32,
        body: Box<WorkflowBlock>,
    },
    /// As Dify's Iteration: run the body once for each item of an array,
    /// which the body reads as `item` and `index` under the Iteration's ID.
    /// Its value is `output`, the body's chosen value of every round.
    Iteration {
        /// The array to iterate.
        iterator: WorkflowValue,
        /// What each round contributes, read after the round.
        output: WorkflowValue,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        is_parallel: bool,
        /// Rounds run at once in parallel mode.
        #[serde(default = "default_parallel_nums")]
        parallel_nums: u32,
        #[serde(
            default,
            skip_serializing_if = "WorkflowIterationErrorMode::is_terminated"
        )]
        error_handle_mode: WorkflowIterationErrorMode,
        /// Join the rounds' arrays into one array.
        #[serde(default = "flatten_by_default", skip_serializing_if = "is_flattened")]
        flatten_output: bool,
        body: Box<WorkflowBlock>,
    },
    /// As Dify's Exit Loop: ends its Loop once this round settles.
    #[serde(rename = "loop_end")]
    LoopEnd {},
    /// As Dify's Variable Assigner: writes variables of the Loop it is in.
    Assigner { items: Vec<WorkflowAssignment> },
    /// As Dify's Human Input: asks the person in the conversation which of
    /// its actions to take. Each action leaves through its own port, and
    /// `timeout` is taken when nobody answers within `timeout_ms`.
    Review {
        /// The Markdown shown to the person: fixed text and references, as
        /// an Agent's task.
        content: Vec<WorkflowValue>,
        actions: Vec<WorkflowReviewAction>,
        timeout_ms: u64,
    },
    /// One statically selected, explicitly admitted foreground capability.
    Tool {
        selector: WorkflowToolSelector,
        arguments: WorkflowValue,
        result: WorkflowToolResult,
        /// Call a failed Tool again before its failure is handled, as Dify's
        /// tool node does; each call is its own invocation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry: Option<WorkflowRetry>,
        /// What a call that still failed leaves behind.
        #[serde(default, skip_serializing_if = "WorkflowValueErrorStrategy::is_stop")]
        on_error: WorkflowValueErrorStrategy,
    },
    /// One execution of an admitted named Subagent profile.
    Agent {
        /// The native named profile to resolve at `AgentRun` admission.
        profile: String,
        /// The task instruction for this `AgentRun`: fixed text and explicit
        /// references, as a Model message's content.
        task: Vec<WorkflowValue>,
        /// What the `AgentRun` returns: its final reply text, or a
        /// structured value committed through `workflow_output`.
        output: WorkflowAgentOutput,
        /// As Dify's Agent output routes: the child picks one of these
        /// exits through its required `switch` value.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        routes: Vec<WorkflowAgentRoute>,
        /// What a failed or timed-out child run leaves behind. Agents are not
        /// retried: a workflow child writes the shared Session workspace
        /// directly, and termination must be proven before it is handled.
        #[serde(default, skip_serializing_if = "WorkflowValueErrorStrategy::is_stop")]
        on_error: WorkflowValueErrorStrategy,
    },
    /// Ordered IF / ELIF cases, followed by the false (ELSE) port.
    Branch { cases: Vec<WorkflowBranchCase> },
    /// As Dify's End: the workflow's result, one field per output variable.
    /// It is the last node; nothing follows it.
    Return { outputs: Vec<WorkflowReturnOutput> },
}

/// As Dify's End output variable: a result field and the variable it reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct WorkflowReturnOutput {
    pub variable: String,
    pub value_selector: Vec<String>,
}

fn empty_object_schema() -> Value {
    serde_json::json!({"type": "object", "properties": {}, "additionalProperties": false})
}

/// Exactly what an omitted schema reads back as.
fn is_default_schema(schema: &Value) -> bool {
    *schema == empty_object_schema()
}

/// A closed object with no fields, however its empty `required` is spelled.
fn is_empty_object_schema(schema: &Value) -> bool {
    let mut schema = schema.clone();
    if schema
        .get("required")
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty)
    {
        schema
            .as_object_mut()
            .map(|object| object.remove("required"));
    }
    schema == empty_object_schema()
}

const fn default_parallel_nums() -> u32 {
    10
}

const fn flatten_by_default() -> bool {
    true
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's `skip_serializing_if` passes a reference.
const fn is_flattened(flatten: &bool) -> bool {
    *flatten
}

/// A Loop variable: Dify's `label`, `var_type` and constant or variable value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowLoopVariable {
    pub label: String,
    pub var_type: WorkflowVarType,
    /// A literal, or a reference read once before the first round.
    pub value: WorkflowValue,
}

/// The value types of a Loop variable, as Dify offers them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub enum WorkflowVarType {
    #[serde(rename = "string")]
    String,
    #[serde(rename = "number")]
    Number,
    #[serde(rename = "boolean")]
    Boolean,
    #[serde(rename = "object")]
    Object,
    #[serde(rename = "array[string]")]
    ArrayString,
    #[serde(rename = "array[number]")]
    ArrayNumber,
    #[serde(rename = "array[boolean]")]
    ArrayBoolean,
    #[serde(rename = "array[object]")]
    ArrayObject,
}

impl WorkflowVarType {
    #[must_use]
    pub fn schema(self) -> Value {
        let scalar = |kind: &str| serde_json::json!({"type": kind});
        let array = |kind: &str| serde_json::json!({"type": "array", "items": {"type": kind}});
        match self {
            Self::String => scalar("string"),
            Self::Number => scalar("number"),
            Self::Boolean => scalar("boolean"),
            Self::Object => scalar("object"),
            Self::ArrayString => array("string"),
            Self::ArrayNumber => array("number"),
            Self::ArrayBoolean => array("boolean"),
            Self::ArrayObject => array("object"),
        }
    }

    /// What `clear` leaves.
    #[must_use]
    pub fn empty(self) -> Value {
        match self {
            Self::String => Value::String(String::new()),
            Self::Number => serde_json::json!(0),
            Self::Boolean => Value::Bool(false),
            Self::Object => serde_json::json!({}),
            _ => serde_json::json!([]),
        }
    }

    const fn is_array(self) -> bool {
        matches!(
            self,
            Self::ArrayString | Self::ArrayNumber | Self::ArrayBoolean | Self::ArrayObject
        )
    }
}

/// The value of a Loop: each of its variables by label.
fn loop_variables_schema(variables: &[WorkflowLoopVariable]) -> Value {
    serde_json::json!({
        "type": "object",
        "properties": variables.iter().map(|variable| (variable.label.clone(), variable.var_type.schema())).collect::<serde_json::Map<_, _>>(),
        "required": variables.iter().map(|variable| variable.label.clone()).collect::<Vec<_>>(),
        "additionalProperties": false,
    })
}

/// How an Iteration treats a round that failed.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowIterationErrorMode {
    /// The Iteration fails.
    #[default]
    Terminated,
    /// The round contributes `null`.
    ContinueOnError,
    /// The round contributes nothing.
    RemoveAbnormalOutput,
}

impl WorkflowIterationErrorMode {
    #[must_use]
    pub const fn is_terminated(&self) -> bool {
        matches!(self, Self::Terminated)
    }
}

/// One write of a Variable Assigner.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowAssignment {
    /// The written Loop variable: `[loop, label]`.
    pub variable: Vec<String>,
    pub operation: WorkflowWriteMode,
    /// The operand; `clear`, `remove_first` and `remove_last` take none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<WorkflowValue>,
}

/// Dify's write modes. `overwrite` writes a variable's value and `set` a
/// constant; both assign.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowWriteMode {
    Overwrite,
    Clear,
    Set,
    Append,
    Extend,
    RemoveFirst,
    RemoveLast,
    Increment,
    Decrement,
    Multiply,
    Divide,
}

impl WorkflowWriteMode {
    /// Whether a variable of `var_type` accepts this mode, as Dify lists them.
    #[must_use]
    pub const fn applies_to(self, var_type: WorkflowVarType) -> bool {
        match self {
            Self::Overwrite | Self::Clear => true,
            Self::Set => !var_type.is_array(),
            Self::Append | Self::Extend | Self::RemoveFirst | Self::RemoveLast => {
                var_type.is_array()
            }
            Self::Increment | Self::Decrement | Self::Multiply | Self::Divide => {
                matches!(var_type, WorkflowVarType::Number)
            }
        }
    }

    /// The operand's schema, or `None` when the mode takes none.
    #[must_use]
    pub fn operand(self, var_type: WorkflowVarType) -> Option<Value> {
        match self {
            Self::Clear | Self::RemoveFirst | Self::RemoveLast => None,
            Self::Append => var_type.schema().get("items").cloned(),
            Self::Increment | Self::Decrement | Self::Multiply | Self::Divide => {
                Some(serde_json::json!({"type": "number"}))
            }
            Self::Overwrite | Self::Set | Self::Extend => Some(var_type.schema()),
        }
    }
}

/// A stable exit identity and its condition. The first matching case wins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowBranchCase {
    pub id: String,
    pub condition: WorkflowPredicate,
}

/// The single authored output contract of a Workflow Model node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowModelOutput {
    /// Preserve the final model text as a string, including JSON-looking text.
    Text {},
    /// Commit only the parsed, locally validated object.
    JsonSchema { schema: Value },
}

impl WorkflowModelOutput {
    /// The derived static node value contract; never separately authored.
    /// As Dify's LLM node: `text`, `reasoning_content` and `usage`, plus
    /// `structured_output` when a schema is declared.
    #[must_use]
    pub fn schema(&self) -> Value {
        let tokens = serde_json::json!({"type": "integer"});
        let mut properties = serde_json::json!({
            "text": {"type": "string"},
            "reasoning_content": {"type": "string"},
            "usage": {
                "type": "object",
                "properties": {"input_tokens": tokens, "output_tokens": tokens, "total_tokens": tokens},
                "required": ["input_tokens", "output_tokens", "total_tokens"],
                "additionalProperties": false,
            },
        });
        let mut required = vec!["text", "reasoning_content", "usage"];
        if let Self::JsonSchema { schema } = self {
            properties["structured_output"] = schema.clone();
            required.push("structured_output");
        }
        serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        })
    }
}

/// The authored output contract of a Workflow Agent node, as Dify's Agent:
/// the final reply is the `text` variable, or a declared schema adds the
/// reserved `workflow_output` terminal and its fields are the variables.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowAgentOutput {
    /// The child's final reply is the result.
    Text {},
    /// The child commits one value matching this schema.
    JsonSchema { schema: Value },
}

impl WorkflowAgentOutput {
    /// The declared schema, when structured output is on.
    #[must_use]
    pub fn structured(&self) -> Option<&Value> {
        match self {
            Self::Text {} => None,
            Self::JsonSchema { schema } => Some(schema),
        }
    }
}

/// One exit an Agent may choose, as Dify's Agent output route. The child
/// sees every route's ID and description when it picks one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowAgentRoute {
    pub id: String,
    pub description: String,
}

/// The reserved field through which a routed child picks its exit.
pub const AGENT_ROUTE_FIELD: &str = "switch";
pub const MAX_AGENT_ROUTES: usize = 32;
pub const MAX_AGENT_ROUTE_DESCRIPTION_BYTES: usize = 1024;

/// As Dify's error handling on its Agent and tool nodes, for a call that
/// failed or timed out once it is proven settled. Cancellation, an unknown
/// outcome, unproven settlement and a refused start still stop the run.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowValueErrorStrategy {
    /// The run stops with the node's failure.
    #[default]
    Stop,
    /// The node succeeds with this value, which satisfies the node's own
    /// value contract; a routed Agent leaves through its `switch`.
    DefaultValue { value: Value },
    /// The node leaves through its `fail` port with `error_type` and
    /// `error_message`; only nodes reached through that port see them.
    FailBranch,
}

impl WorkflowValueErrorStrategy {
    #[must_use]
    pub const fn is_stop(&self) -> bool {
        matches!(self, Self::Stop)
    }
}

fn agent_text_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {"text": {"type": "string"}},
        "required": ["text"],
        "additionalProperties": false,
    })
}

/// The schema the child's `workflow_output` terminal must satisfy, when it
/// has one: the declared schema, or `{text}` once routes need a commit,
/// plus the required `switch` naming one route.
#[must_use]
pub fn agent_terminal_schema(
    output: &WorkflowAgentOutput,
    routes: &[WorkflowAgentRoute],
) -> Option<Value> {
    let mut schema = match (output.structured(), routes.is_empty()) {
        (None, true) => return None,
        (None, false) => agent_text_schema(),
        (Some(schema), _) => schema.clone(),
    };
    if !routes.is_empty() {
        let choices = routes
            .iter()
            .map(|route| format!("- {}: {}", route.id, route.description))
            .collect::<Vec<_>>()
            .join("\n");
        schema["properties"][AGENT_ROUTE_FIELD] = serde_json::json!({
            "type": "string",
            "enum": routes.iter().map(|route| route.id.as_str()).collect::<Vec<_>>(),
            "description": format!("Select exactly one route ID based on the task result:\n{choices}"),
        });
        match schema.get_mut("required").and_then(Value::as_array_mut) {
            Some(required) => required.push(AGENT_ROUTE_FIELD.into()),
            None => schema["required"] = serde_json::json!([AGENT_ROUTE_FIELD]),
        }
    }
    Some(schema)
}

/// The derived static node value contract: the terminal schema, or `{text}`
/// for a plain final reply.
#[must_use]
pub fn agent_value_schema(output: &WorkflowAgentOutput, routes: &[WorkflowAgentRoute]) -> Value {
    agent_terminal_schema(output, routes).unwrap_or_else(agent_text_schema)
}

/// As Dify's retry on failure: a failed or timed-out Model or Tool call is
/// made again, up to `max_retries` more times, `interval_ms` after the
/// previous one ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowRetry {
    pub max_retries: u32,
    pub interval_ms: u32,
}

/// As Dify's error handling, for a call that failed or timed out after its
/// retries. Cancellation, an unknown outcome and authority errors still stop
/// the run.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowModelErrorStrategy {
    /// The run stops with the node's failure.
    #[default]
    Stop,
    /// The node succeeds with this `text` (and `structured_output`, which a
    /// declared schema requires), empty reasoning and zero usage.
    DefaultValue {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        structured_output: Option<Value>,
    },
    /// The node leaves through its `fail` port with `error_type` and
    /// `error_message`; only nodes reached through that port see them.
    FailBranch,
}

impl WorkflowModelErrorStrategy {
    #[must_use]
    pub const fn is_stop(&self) -> bool {
        matches!(self, Self::Stop)
    }

    /// The node value a consumer on the `fail` path sees.
    #[must_use]
    pub fn error_schema() -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {"error_type": {"type": "string"}, "error_message": {"type": "string"}},
            "required": ["error_type", "error_message"],
            "additionalProperties": false,
        })
    }
}

/// The Dify retry bounds.
pub const MAX_RETRIES: u32 = 10;
pub const RETRY_INTERVAL_MS: std::ops::RangeInclusive<u32> = 100..=5000;

/// One prompt message of a Model node. The first message is the system
/// prompt and every later one a user or assistant turn. Its content is fixed
/// text (string literals) and explicit references, rendered in order: a
/// string value as itself, any other value as its JSON text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowModelMessage {
    pub role: WorkflowModelRole,
    #[serde(default)]
    pub content: Vec<WorkflowValue>,
}

/// Who speaks a Model prompt message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowModelRole {
    System,
    User,
    Assistant,
}

/// Closed, explicitly tagged value syntax; no evaluation of expression strings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
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
    /// One string from fixed text and references, rendered as prompt parts
    /// are: a non-string referenced value is inserted as its JSON text.
    Template { parts: Vec<WorkflowValue> },
}

/// Typed predicates. Equality requires matching scalar types; no coercion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub enum WorkflowPredicate {
    /// A boolean value.
    Boolean {
        value: WorkflowValue,
    },
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
    /// String containment, without coercion.
    Contains {
        left: WorkflowValue,
        right: WorkflowValue,
    },
    NotContains {
        left: WorkflowValue,
        right: WorkflowValue,
    },
    StartsWith {
        left: WorkflowValue,
        right: WorkflowValue,
    },
    EndsWith {
        left: WorkflowValue,
        right: WorkflowValue,
    },
    /// Whether a string is empty.
    Empty {
        value: WorkflowValue,
    },
    NotEmpty {
        value: WorkflowValue,
    },
    /// Boolean negation.
    Not {
        predicate: Box<WorkflowPredicate>,
    },
    /// Conjunction of a nonempty fixed list.
    And {
        predicates: Vec<WorkflowPredicate>,
    },
    /// Disjunction of a nonempty fixed list.
    Or {
        predicates: Vec<WorkflowPredicate>,
    },
}

/// One explicit edge in the workflow graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct WorkflowEdgeDefinition {
    /// Source node id.
    pub from: String,
    /// Destination node id.
    pub to: String,
    /// The Branch case, Agent route or fail port. Ordinary nodes omit it.
    #[serde(default)]
    pub port: Option<WorkflowPort>,
}

/// A control-flow port of a fixed Workflow node.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WorkflowPort {
    /// The true successor.
    True,
    /// The false successor.
    False,
    /// An additional named ELIF exit.
    Case(String),
    /// The ordinary outgoing port.
    Next,
    /// A Model's failure exit under its fail-branch strategy.
    Fail,
}

impl WorkflowPort {
    pub fn case(id: &str) -> Self {
        if id == "true" {
            Self::True
        } else {
            Self::Case(id.to_owned())
        }
    }
}

impl Serialize for WorkflowPort {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(match self {
            Self::True => "true",
            Self::False => "false",
            Self::Next => "next",
            Self::Fail => "fail",
            Self::Case(id) => id,
        })
    }
}

impl schemars::JsonSchema for WorkflowPort {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "WorkflowPort".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        serde_json::json!({"anyOf":[{"type":"boolean"},{"type":"string","minLength":1,"maxLength":64,"pattern":"^[A-Za-z0-9_-]+$"}]})
            .try_into()
            .expect("schema object")
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
                "fail" => Ok(Self::Fail),
                _ if valid_local_key(&text) => Ok(Self::Case(text)),
                _ => Err(serde::de::Error::custom("invalid workflow edge port")),
            },
        }
    }
}

/// A compiled, immutable executable workflow.
#[derive(Debug, Clone)]
pub struct WorkflowProgram {
    digest: String,
    id: WorkflowId,
    description: String,
    block: WorkflowBlockProgram,
    execution_bound: usize,
    agent_bound: usize,
    model_call_bound: usize,
    retained_bound: usize,
    tools: BTreeSet<crate::capabilities::selection::ExactToolSelector>,
    timeout_ms: u64,
    frozen_tools: BTreeMap<
        crate::capabilities::selection::ExactToolSelector,
        crate::tools::types::ToolDefinition,
    >,
}

/// Immutable compiled graph of the root or of a body; a body's path is not empty.
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
    /// schemas, references, or override authoring are invalid.
    pub fn compile(
        id: WorkflowId,
        definition: WorkflowDefinition,
    ) -> Result<Self, WorkflowCompileError> {
        compile_program(id, definition)
    }

    /// The configured identity.
    #[must_use]
    pub fn id(&self) -> &WorkflowId {
        &self.id
    }

    /// Bounded immutable identity, with no executable state.
    #[must_use]
    pub fn tool_identity(&self) -> WorkflowToolIdentity {
        WorkflowToolIdentity {
            workflow_id: self.id.clone(),
            program_digest: self.digest.clone(),
        }
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

    pub(crate) fn agent_nodes(&self) -> Vec<&WorkflowAgentProgram> {
        fn walk<'a>(block: &'a WorkflowBlockProgram, found: &mut Vec<&'a WorkflowAgentProgram>) {
            for node in block.nodes.values() {
                match node {
                    WorkflowNodeProgram::Agent(agent) => found.push(agent),
                    WorkflowNodeProgram::Loop { body, .. }
                    | WorkflowNodeProgram::Iteration { body, .. } => walk(body, found),
                    _ => {}
                }
            }
        }
        let mut found = Vec::new();
        walk(&self.block, &mut found);
        found
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
    Start,
    /// One immutable, tool-free Auxiliary Call template.
    Model(WorkflowModelProgram),
    Loop {
        loop_variables: Vec<WorkflowLoopVariable>,
        break_condition: Option<WorkflowPredicate>,
        loop_count: u32,
        body: Box<WorkflowBlockProgram>,
        output_schema: Value,
    },
    Iteration {
        iterator: WorkflowValue,
        output: WorkflowValue,
        /// Rounds run at once; one when sequential.
        parallel: u32,
        error_handle_mode: WorkflowIterationErrorMode,
        flatten_output: bool,
        body: Box<WorkflowBlockProgram>,
        output_schema: Value,
    },
    LoopEnd,
    Assigner {
        items: Vec<WorkflowAssignment>,
        /// Each written variable's type.
        types: Vec<WorkflowVarType>,
    },
    Review {
        content: Vec<WorkflowValue>,
        actions: Vec<WorkflowReviewAction>,
        timeout_ms: u64,
    },
    Tool {
        selector: crate::capabilities::selection::ExactToolSelector,
        arguments: WorkflowValue,
        result: WorkflowToolResult,
        retry: Option<WorkflowRetry>,
        on_error: WorkflowValueErrorStrategy,
    },
    /// One admitted `AgentRun` template.
    Agent(WorkflowAgentProgram),
    /// The first matching IF / ELIF case, or ELSE.
    Branch {
        cases: Vec<WorkflowBranchCase>,
    },
    /// The terminal Return operation.
    Return {
        output: WorkflowValue,
    },
}

/// Static Model intent; actual invocation resolution uses frozen Attempt resources.
#[derive(Debug, Clone)]
pub struct WorkflowModelProgram {
    pub model: crate::model::session::AuxiliaryModelPolicy,
    pub messages: Vec<WorkflowModelMessage>,
    pub output: WorkflowModelOutput,
    pub retry: Option<WorkflowRetry>,
    pub on_error: WorkflowModelErrorStrategy,
}

/// A compiled `AgentRun` template.
#[derive(Debug, Clone)]
pub struct WorkflowAgentProgram {
    pub(crate) resolved: Option<Arc<crate::runtime::subagent::resolver::FrozenAgentComposition>>,
    /// The admitted native profile.
    pub profile: SubagentName,
    /// The task's fixed text and references.
    pub task: Vec<WorkflowValue>,
    /// Frozen output contract.
    pub output: WorkflowAgentOutput,
    /// The exits the child chooses among; empty for one `next` exit.
    pub routes: Vec<WorkflowAgentRoute>,
    pub on_error: WorkflowValueErrorStrategy,
}

impl WorkflowAgentProgram {
    /// The schema of the child's `workflow_output` terminal, when it has one.
    #[must_use]
    pub fn terminal_schema(&self) -> Option<Value> {
        agent_terminal_schema(&self.output, &self.routes)
    }

    /// The node's value contract on its success exits.
    #[must_use]
    pub fn value_schema(&self) -> Value {
        agent_value_schema(&self.output, &self.routes)
    }

    /// The node value of one settled child: its reply text, or its
    /// committed value.
    fn value(&self, settled: Value) -> Option<Value> {
        match (self.terminal_schema(), settled) {
            (None, Value::String(text)) => Some(serde_json::json!({"text": text})),
            (Some(_), value) if value.is_object() => Some(value),
            _ => None,
        }
    }

    /// The route a validated routed value chose.
    pub(super) fn chosen_route(value: &Value) -> WorkflowPort {
        WorkflowPort::case(value[AGENT_ROUTE_FIELD].as_str().unwrap_or_default())
    }
}

/// A compiled edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowEdgeProgram {
    /// Destination id.
    pub to: String,
    /// The port used by this edge.
    pub port: WorkflowPort,
}

/// A discovered source and its generation-owned admission decision.
#[derive(Debug, Clone)]
pub struct WorkflowEntry {
    pub source: Arc<WorkflowProgram>,
    pub admission: WorkflowAdmission,
}

/// Only Enabled carries a frozen executable program. Discovery alone is inert.
#[derive(Debug, Clone)]
pub enum WorkflowAdmission {
    Enabled(Arc<WorkflowProgram>),
    Disabled(Vec<WorkflowAdmissionDiagnostic>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkflowAdmissionDiagnostic {
    pub path: String,
    pub reason: WorkflowDependencyFailure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
#[derive(schemars::JsonSchema)]
pub enum WorkflowDependencyFailure {
    NotAdmitted,
    /// Bounded static model error classification, without provider details.
    Model {
        reason: String,
    },
    Materialization {
        #[serde(skip)]
        detail: String,
    },
    Agent(crate::runtime::agent_profile::AgentProfileDiagnostic),
    Tool(crate::capabilities::selection::ToolSelectionError),
    IneligibleTool(crate::capabilities::selection::ExactToolSelector),
}

impl WorkflowAdmissionDiagnostic {
    pub(crate) fn redacted(&self) -> Self {
        let mut fact = self.clone();
        match &mut fact.reason {
            WorkflowDependencyFailure::Agent(reason) => *reason = reason.redacted(),
            WorkflowDependencyFailure::Tool(reason) => *reason = reason.redacted(),
            WorkflowDependencyFailure::Materialization { detail } => detail.clear(),
            _ => {}
        }
        fact
    }
}

/// One immutable catalog; enabled identities are derived, never separately authored.
#[derive(Debug, Clone, Default)]
pub struct WorkflowCatalog {
    pub(crate) locations: BTreeMap<WorkflowId, crate::runtime::resources::ResourceLocation>,
    pub(crate) discovery_diagnostics: Vec<crate::runtime::resources::RuntimeResourceLoadError>,
    /// Source diagnostics only: never part of a program, Tool schema or run.
    pub(crate) presentation_diagnostics:
        BTreeMap<WorkflowId, Vec<crate::runtime::resources::RuntimeResourceLoadError>>,
    entries: BTreeMap<WorkflowId, WorkflowEntry>,
    invalid: BTreeMap<WorkflowId, crate::runtime::resources::RuntimeResourceLoadError>,
}

impl WorkflowCatalog {
    pub(crate) fn set_invalid(
        &mut self,
        invalid: BTreeMap<WorkflowId, crate::runtime::resources::RuntimeResourceLoadError>,
    ) {
        self.invalid = invalid;
    }
    #[must_use]
    pub fn invalid(
        &self,
    ) -> &BTreeMap<WorkflowId, crate::runtime::resources::RuntimeResourceLoadError> {
        &self.invalid
    }

    /// Discovers structurally compiled sources. Admission is a separate candidate phase.
    ///
    /// # Errors
    /// Rejects duplicate identities or an oversized catalog.
    pub fn new(
        programs: impl IntoIterator<Item = WorkflowProgram>,
    ) -> Result<Self, WorkflowCatalogError> {
        let mut entries = BTreeMap::new();
        for program in programs {
            if entries
                .insert(
                    program.id.clone(),
                    WorkflowEntry {
                        source: Arc::new(program),
                        admission: WorkflowAdmission::Disabled(vec![WorkflowAdmissionDiagnostic {
                            path: "block".into(),
                            reason: WorkflowDependencyFailure::NotAdmitted,
                        }]),
                    },
                )
                .is_some()
            {
                return Err(WorkflowCatalogError::DuplicateDefinition);
            }
            if entries.len() > MAX_WORKFLOW_DEFINITIONS {
                return Err(WorkflowCatalogError::TooManyDefinitions);
            }
        }
        Ok(Self {
            entries,
            locations: BTreeMap::new(),
            invalid: BTreeMap::new(),
            discovery_diagnostics: Vec::new(),
            presentation_diagnostics: BTreeMap::new(),
        })
    }

    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Discovered source, including disabled programs, for inert inspection.
    #[must_use]
    pub fn get(&self, id: &WorkflowId) -> Option<&Arc<WorkflowProgram>> {
        self.entries.get(id).map(|entry| &entry.source)
    }

    #[must_use]
    pub fn entries(&self) -> &BTreeMap<WorkflowId, WorkflowEntry> {
        &self.entries
    }

    #[must_use]
    pub fn enabled_ids(&self) -> BTreeSet<WorkflowId> {
        self.entries
            .iter()
            .filter_map(|(id, entry)| {
                matches!(entry.admission, WorkflowAdmission::Enabled(_)).then_some(id.clone())
            })
            .collect()
    }

    /// The common direct/native execution gate; it never rediscovers dependencies.
    ///
    /// # Errors
    /// Returns the generation's retained disabled reason or an unknown identity.
    pub fn executable(&self, id: &WorkflowId) -> Result<&Arc<WorkflowProgram>, WorkflowRunError> {
        match self.entries.get(id) {
            Some(WorkflowEntry {
                admission: WorkflowAdmission::Enabled(program),
                ..
            }) => Ok(program),
            Some(WorkflowEntry {
                admission: WorkflowAdmission::Disabled(diagnostics),
                ..
            }) => Err(WorkflowRunError::Disabled {
                workflow: id.clone(),
                diagnostics: diagnostics.clone(),
            }),
            None => Err(WorkflowRunError::InvalidProgram(format!(
                "Workflow {id} is not discovered"
            ))),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowCatalogError {
    DuplicateDefinition,
    TooManyDefinitions,
}
impl fmt::Display for WorkflowCatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::DuplicateDefinition => "duplicate workflow definition id",
            Self::TooManyDefinitions => "too many Workflow definitions",
        })
    }
}
impl std::error::Error for WorkflowCatalogError {}

/// A compile-time graph/type/reference rejection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowCompileError {
    /// Authored field context supplied by the compiler, never inferred from prose.
    At { path: String, cause: Box<Self> },
    /// A required textual field is empty or over its bound.
    InvalidField(String),
    /// A JSON Schema is invalid or not a root object schema.
    InvalidSchema(String),
    /// A node or edge refers to an unknown id.
    DanglingReference(String),
    /// The graph has no single explicit entry.
    InvalidEntry(String),
    /// The graph contains a cycle; node is the smallest positive-indegree Kahn residual.
    Cycle { node: String },
    /// A node is unreachable from the explicit entry.
    Unreachable(String),
    /// A path can leave a node without a deterministic successor/terminal.
    Unterminated(String),
    /// A Branch has invalid cases or an incomplete set of exit connections.
    InvalidBranch(String),
    /// A workflow reference cannot be resolved on every relevant path.
    InvalidReference(String),
    /// A reference has an incompatible statically known schema.
    IncompatibleReference(String),
}

impl fmt::Display for WorkflowCompileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::At { path, cause } => write!(formatter, "{path}: {cause}"),
            Self::InvalidField(detail)
            | Self::InvalidSchema(detail)
            | Self::DanglingReference(detail)
            | Self::InvalidEntry(detail)
            | Self::Unterminated(detail)
            | Self::InvalidBranch(detail)
            | Self::InvalidReference(detail)
            | Self::IncompatibleReference(detail) => formatter.write_str(detail),
            Self::Cycle { node } => write!(
                formatter,
                "workflow graph contains a cycle; residual node {node:?}"
            ),
            Self::Unreachable(node) => write!(formatter, "workflow node {node:?} is unreachable"),
        }
    }
}

impl std::error::Error for WorkflowCompileError {}

impl WorkflowCompileError {
    pub(super) fn at(self, prefix: impl AsRef<str>) -> Self {
        match self {
            Self::At { path, cause } => Self::At {
                path: format!("{}.{path}", prefix.as_ref()),
                cause,
            },
            cause => Self::At {
                path: prefix.as_ref().into(),
                cause: Box::new(cause),
            },
        }
    }

    /// The deepest native compiler rejection, independent of authored context.
    #[must_use]
    pub fn cause(&self) -> &Self {
        match self {
            Self::At { cause, .. } => cause.cause(),
            cause => cause,
        }
    }

    /// Authored field path, supplied by the validation owner.
    #[must_use]
    pub fn path(&self) -> &str {
        match self {
            Self::At { path, .. } => path,
            _ => "$",
        }
    }
}

#[allow(clippy::too_many_lines)] // one bounded graph-to-program validation pipeline
fn compile_program(
    id: WorkflowId,
    definition: WorkflowDefinition,
) -> Result<WorkflowProgram, WorkflowCompileError> {
    if definition.timeout_ms == 0 || definition.timeout_ms > 86_400_000 {
        return Err(
            WorkflowCompileError::InvalidField("timeout_ms must be 1..=86400000".into())
                .at("timeout_ms"),
        );
    }
    if definition.description.trim().is_empty() {
        return Err(WorkflowCompileError::InvalidField(format!(
            "workflow {id} has an empty description"
        ))
        .at("description"));
    }
    if definition.description.len() > 4096 {
        return Err(WorkflowCompileError::InvalidField(format!(
            "workflow {id} description is too large"
        ))
        .at("description"));
    }
    if serde_json::to_vec(&definition).map_or(true, |bytes| bytes.len() > MAX_WORKFLOW_BYTES) {
        return Err(WorkflowCompileError::InvalidField(
            "aggregate program size exceeded".into(),
        ));
    }
    let digest = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&definition).expect("validated definition"))
    );
    let mut total_nodes = 0;
    let block = compile_block(definition.block, Vec::new(), &mut total_nodes, None)
        .map_err(|error| error.at("block"))?;
    let retained_bound = execution::static_retained_bound(&block);
    let (execution_bound, agent_bound, model_call_bound) =
        execution::static_execution_bound(&block).map_err(|e| e.at("block"))?;
    if retained_bound > MAX_LOCAL_BYTES {
        return Err(WorkflowCompileError::InvalidField(
            "aggregate retained-data reservation exceeds the run budget".into(),
        )
        .at("block"));
    }
    Ok(WorkflowProgram {
        digest,
        id,
        description: definition.description,
        tools: collect_tool_selectors(&block),
        block,
        execution_bound,
        agent_bound,
        model_call_bound,
        retained_bound,
        timeout_ms: definition.timeout_ms,
        frozen_tools: BTreeMap::new(),
    })
}

fn collect_tool_selectors(
    block: &WorkflowBlockProgram,
) -> BTreeSet<crate::capabilities::selection::ExactToolSelector> {
    let mut selectors = BTreeSet::new();
    for node in block.nodes.values() {
        match node {
            WorkflowNodeProgram::Tool { selector, .. } => {
                selectors.insert(selector.clone());
            }
            WorkflowNodeProgram::Loop { body, .. }
            | WorkflowNodeProgram::Iteration { body, .. } => {
                selectors.extend(collect_tool_selectors(body));
            }
            _ => {}
        }
    }
    selectors
}

/// What a Loop or Iteration body sees around it.
struct Container {
    /// The variables in scope at the container, plus its own.
    base: SchemaMap,
    owner: String,
    /// A Loop's variables by label; `None` in an Iteration.
    loop_variables: Option<BTreeMap<String, WorkflowVarType>>,
}

/// A body and the scope after it: every variable a round may have set,
/// for the Loop's break condition and the Iteration's output.
fn compile_body(
    body: &WorkflowBlock,
    path: &[String],
    owner: &str,
    total_nodes: &mut usize,
    container: &Container,
) -> Result<(WorkflowBlockProgram, SchemaMap), WorkflowCompileError> {
    if !is_empty_object_schema(&body.input) {
        return Err(WorkflowCompileError::InvalidField(
            "a body declares no input; it reads the variables around it".into(),
        )
        .at("body"));
    }
    let mut child_path = path.to_vec();
    child_path.extend([owner.to_owned(), "body".into()]);
    let program = compile_block(body.clone(), child_path, total_nodes, Some(container))
        .map_err(|e| e.at("body"))?;
    let mut end = container.base.clone();
    for (id, node) in &body.nodes {
        if let Ok(Some(schema)) = authoring::declared_output(node) {
            end.0.insert(id.clone(), schema);
        }
    }
    Ok((program, end))
}

#[allow(clippy::too_many_lines)]
fn compile_block(
    definition: WorkflowBlock,
    path: Vec<String>,
    total_nodes: &mut usize,
    container: Option<&Container>,
) -> Result<WorkflowBlockProgram, WorkflowCompileError> {
    *total_nodes += definition.nodes.len();
    if path.len() / 2 > MAX_BLOCK_DEPTH || *total_nodes > MAX_WORKFLOW_NODES {
        return Err(WorkflowCompileError::InvalidField(
            "aggregate block depth/node bound exceeded".into(),
        ));
    }
    validate_root_schema(&definition.input, "input").map_err(|e| e.at("input"))?;
    let base = container.map_or_else(
        || SchemaMap::from_schema(&definition.input),
        |container| container.base.clone(),
    );
    // As Dify's node IDs, a body's IDs never hide a variable around it.
    if let Some(shadowed) = definition.nodes.keys().find(|id| base.0.contains_key(*id)) {
        return Err(WorkflowCompileError::InvalidField(format!(
            "node id {shadowed:?} is already used around this body"
        ))
        .at(format!("nodes.{shadowed}")));
    }
    let ControlFlow {
        outgoing,
        topological,
        entry,
        scopes,
    } = control_flow(&definition)?;

    let mut nodes = BTreeMap::new();
    let mut available_after: BTreeMap<String, SchemaMap> = BTreeMap::new();
    let mut returned = Vec::new();
    for node_id in topological {
        let node = definition
            .nodes
            .get(&node_id)
            .expect("topological node exists");
        let available_before = scopes.scope(&node_id, &available_after, &base);
        let compiled = (|| { Ok(match node {
            WorkflowNodeDefinition::Start {} => {
                available_after.insert(node_id.clone(), available_before.clone());
                WorkflowNodeProgram::Start
            }
            WorkflowNodeDefinition::Model { model, messages, output, retry, on_error } => {
                let model = model.lower().map_err(|error| error.at("model"))?;
                validate_model(&model, messages, output, &available_before, &node_id)?;
                validate_model_failure(retry.as_ref(), on_error, output, &node_id)?;
                available_after.insert(
                    node_id.clone(),
                    available_before.with_prefix(&node_id, &output.schema()),
                );
                WorkflowNodeProgram::Model(WorkflowModelProgram {
                    model,
                    messages: messages.clone(),
                    output: output.clone(),
                    retry: *retry,
                    on_error: on_error.clone(),
                })
            }
            WorkflowNodeDefinition::Loop {
                loop_variables,
                break_condition,
                loop_count,
                body,
            } => {
                if *loop_count == 0 || *loop_count > LOOP_NODE_MAX_COUNT {
                    return Err(WorkflowCompileError::InvalidField(format!(
                        "Loop {node_id:?} loop_count must be 1..={LOOP_NODE_MAX_COUNT}"
                    )).at("loop_count"));
                }
                let mut labels = BTreeSet::new();
                for (index, variable) in loop_variables.iter().enumerate() {
                    if !valid_local_key(&variable.label) || !labels.insert(&variable.label) {
                        return Err(WorkflowCompileError::InvalidField(format!(
                            "Loop {node_id:?} variable labels must be unique valid keys"
                        )).at(format!("loop_variables.{index}.label")));
                    }
                    let actual = value_schema(&variable.value, &available_before, &node_id, 0)
                        .map_err(|e| e.at(format!("loop_variables.{index}.value")))?;
                    if !schemas_compatible(&actual, &variable.var_type.schema()) {
                        return Err(WorkflowCompileError::IncompatibleReference(format!(
                            "Loop variable {:?} value is not a {:?}", variable.label, variable.var_type
                        )).at(format!("loop_variables.{index}.value")));
                    }
                }
                let output_schema = loop_variables_schema(loop_variables);
                let container = Container {
                    base: available_before.with_prefix(&node_id, &output_schema),
                    owner: node_id.clone(),
                    loop_variables: Some(loop_variables.iter().map(|variable| (variable.label.clone(), variable.var_type)).collect()),
                };
                let (body, end) = compile_body(body, &path, &node_id, total_nodes, &container)?;
                if let Some(condition) = break_condition {
                    validate_predicate(condition, &end, &node_id, 0).map_err(|e| e.at("break_condition"))?;
                }
                available_after.insert(
                    node_id.clone(),
                    available_before.with_prefix(&node_id, &output_schema),
                );
                WorkflowNodeProgram::Loop {
                    loop_variables: loop_variables.clone(),
                    break_condition: break_condition.clone(),
                    loop_count: *loop_count,
                    body: Box::new(body),
                    output_schema,
                }
            }
            WorkflowNodeDefinition::Iteration {
                iterator,
                output,
                is_parallel,
                parallel_nums,
                error_handle_mode,
                flatten_output,
                body,
            } => {
                if !matches!(iterator, WorkflowValue::Reference { .. }) {
                    return Err(WorkflowCompileError::InvalidField(
                        "Iteration input must reference an array".into(),
                    ).at("iterator"));
                }
                let array = value_schema(iterator, &available_before, &node_id, 0).map_err(|e| e.at("iterator"))?;
                let Some(item) = array.get("items").filter(|_| schema_type(&array) == Some("array")) else {
                    return Err(WorkflowCompileError::IncompatibleReference(
                        "Iteration input must be an array with declared items".into(),
                    ).at("iterator"));
                };
                if *is_parallel && (*parallel_nums == 0 || *parallel_nums > MAX_PARALLEL_LIMIT) {
                    return Err(WorkflowCompileError::InvalidField(format!(
                        "Iteration {node_id:?} parallel_nums must be 1..={MAX_PARALLEL_LIMIT}"
                    )).at("parallel_nums"));
                }
                if !matches!(output, WorkflowValue::Reference { .. }) {
                    return Err(WorkflowCompileError::InvalidField(
                        "Iteration output must reference a variable of its body".into(),
                    ).at("output"));
                }
                let entry = serde_json::json!({
                    "type": "object",
                    "properties": {"item": item, "index": {"type": "integer"}},
                    "required": ["item", "index"],
                    "additionalProperties": false,
                });
                let container = Container {
                    base: available_before.with_prefix(&node_id, &entry),
                    owner: node_id.clone(),
                    loop_variables: None,
                };
                let (body, end) = compile_body(body, &path, &node_id, total_nodes, &container)?;
                let produced = value_schema(output, &end, &node_id, 0).map_err(|e| e.at("output"))?;
                let output_schema = execution::iteration_result_schema(Some(&produced), *error_handle_mode, *flatten_output);
                available_after.insert(
                    node_id.clone(),
                    available_before.with_prefix(&node_id, &output_schema),
                );
                WorkflowNodeProgram::Iteration {
                    iterator: iterator.clone(),
                    output: output.clone(),
                    parallel: if *is_parallel { *parallel_nums } else { 1 },
                    error_handle_mode: *error_handle_mode,
                    flatten_output: *flatten_output,
                    body: Box::new(body),
                    output_schema,
                }
            }
            WorkflowNodeDefinition::LoopEnd {} => {
                if container.is_none_or(|container| container.loop_variables.is_none()) {
                    return Err(WorkflowCompileError::InvalidField(
                        "Exit Loop belongs directly in a Loop body".into(),
                    ));
                }
                if !outgoing[&node_id].is_empty() {
                    return Err(WorkflowCompileError::Unterminated(format!(
                        "Exit Loop {node_id:?} cannot have outgoing edges"
                    )));
                }
                available_after.insert(node_id.clone(), available_before.clone());
                WorkflowNodeProgram::LoopEnd
            }
            WorkflowNodeDefinition::Assigner { items } => {
                let Some((owner, variables)) = container.and_then(|container| Some((&container.owner, container.loop_variables.as_ref()?))) else {
                    return Err(WorkflowCompileError::InvalidField(
                        "a Variable Assigner writes Loop variables, so it belongs directly in a Loop body".into(),
                    ));
                };
                if items.is_empty() || items.len() > 32 {
                    return Err(WorkflowCompileError::InvalidField(
                        "a Variable Assigner makes 1..=32 writes".into(),
                    ).at("items"));
                }
                let mut types = Vec::new();
                for (index, item) in items.iter().enumerate() {
                    let var_type = match item.variable.as_slice() {
                        [loop_id, label] if loop_id == owner => variables.get(label).copied(),
                        _ => None,
                    }.ok_or_else(|| WorkflowCompileError::DanglingReference(
                        "an assigned variable must be a variable of the Loop the Assigner is in".into(),
                    ).at(format!("items.{index}.variable")))?;
                    if !item.operation.applies_to(var_type) {
                        return Err(WorkflowCompileError::InvalidField(format!(
                            "{:?} cannot write a {var_type:?} variable", item.operation
                        )).at(format!("items.{index}.operation")));
                    }
                    match (item.operation.operand(var_type), &item.value) {
                        (None, None) => {}
                        (Some(expected), Some(value)) => {
                            let actual = value_schema(value, &available_before, &node_id, 0).map_err(|e| e.at(format!("items.{index}.value")))?;
                            if !schemas_compatible(&actual, &expected) {
                                return Err(WorkflowCompileError::IncompatibleReference(format!(
                                    "{:?} needs a value of {expected}", item.operation
                                )).at(format!("items.{index}.value")));
                            }
                        }
                        (Some(_), None) => return Err(WorkflowCompileError::InvalidField(
                            "this write needs a value".into(),
                        ).at(format!("items.{index}.value"))),
                        (None, Some(_)) => return Err(WorkflowCompileError::InvalidField(
                            "this write takes no value".into(),
                        ).at(format!("items.{index}.value"))),
                    }
                    types.push(var_type);
                }
                available_after.insert(node_id.clone(), available_before.clone());
                WorkflowNodeProgram::Assigner { items: items.clone(), types }
            }
            WorkflowNodeDefinition::Review { content, actions, timeout_ms } => {
                validate_review(content, actions, *timeout_ms, &available_before, &node_id)?;
                let exits = actions.iter().map(|action| WorkflowPort::case(&action.id))
                    .chain(std::iter::once(WorkflowPort::case(REVIEW_TIMEOUT_PORT)));
                if container.is_none() && exits.into_iter().any(|port| !outgoing[&node_id].iter().any(|edge| edge.port == port)) {
                    return Err(WorkflowCompileError::InvalidBranch(format!(
                        "Human Input {node_id:?} requires a successor for every action and its timeout"
                    )));
                }
                available_after.insert(
                    node_id.clone(),
                    available_before.with_prefix(&node_id, &review::result_schema(actions)),
                );
                WorkflowNodeProgram::Review {
                    content: content.clone(),
                    actions: actions.clone(),
                    timeout_ms: *timeout_ms,
                }
            }
            WorkflowNodeDefinition::Tool {
                selector,
                arguments,
                result,
                retry,
                on_error,
            } => {
                let selector = selector.lower().map_err(|error| error.at("selector"))?;
                if let crate::capabilities::selection::ExactToolSelector::Builtin { name } = &selector
                    && let Some(extension) = crate::capabilities::extension_provided_tool(name)
                {
                    return Err(WorkflowCompileError::InvalidField(format!(
                        "builtin:{name} belongs to Agent Extension {extension}, not ordinary Tool selection"
                    )).at("selector"));
                }
                let input = value_schema(arguments, &available_before, &node_id, 0).map_err(|e| e.at("arguments"))?;
                validate_tool_arguments_root(&input).map_err(|e| e.at("arguments"))?;
                let output = result.schema();
                validate_workflow_schema(&output, "Tool result").map_err(|e| e.at("result.schema"))?;
                validate_retry(retry.as_ref(), "Tool", &node_id)?;
                if let WorkflowValueErrorStrategy::DefaultValue { value } = on_error
                    && !jsonschema::Validator::new(&output).is_ok_and(|validator| validator.is_valid(value))
                {
                    return Err(WorkflowCompileError::InvalidField(format!(
                        "Tool {node_id:?} default value must satisfy its result schema"
                    ))
                    .at("on_error.value"));
                }
                available_after.insert(
                    node_id.clone(),
                    available_before.with_prefix(&node_id, &output),
                );
                WorkflowNodeProgram::Tool {
                    selector,
                    arguments: arguments.clone(),
                    result: result.clone(),
                    retry: *retry,
                    on_error: on_error.clone(),
                }
            }
            WorkflowNodeDefinition::Agent {
                profile,
                task,
                output,
                routes,
                on_error,
            } => {
                let profile = SubagentName::parse(profile).map_err(|_| WorkflowCompileError::InvalidField("Agent selection requires a valid named profile".into()).at("profile"))?;
                validate_agent(task, output, routes, on_error, &available_before, &node_id)?;
                let exits = routes.iter().map(|route| WorkflowPort::case(&route.id))
                    .chain(on_error.eq(&WorkflowValueErrorStrategy::FailBranch).then_some(WorkflowPort::Fail));
                if container.is_none() && exits.into_iter().any(|port| !outgoing[&node_id].iter().any(|edge| edge.port == port)) {
                    return Err(WorkflowCompileError::InvalidBranch(format!(
                        "Agent {node_id:?} requires a successor for every route and its fail branch"
                    )));
                }
                let agent = WorkflowAgentProgram {
                    resolved: None,
                    profile,
                    task: task.clone(),
                    output: output.clone(),
                    routes: routes.clone(),
                    on_error: on_error.clone(),
                };
                available_after.insert(
                    node_id.clone(),
                    available_before.with_prefix(&node_id, &agent.value_schema()),
                );
                WorkflowNodeProgram::Agent(agent)
            }
            WorkflowNodeDefinition::Branch { cases } => {
                for (index, case) in cases.iter().enumerate() {
                    validate_predicate(&case.condition, &available_before, &node_id, 0)
                        .map_err(|e| e.at(format!("cases.{index}.condition")))?;
                }
                let ports = cases.iter().map(|case| WorkflowPort::case(&case.id))
                    .chain(std::iter::once(WorkflowPort::False));
                // A body's path may end at any port, as Dify's.
                if container.is_none() && ports.into_iter().any(|port| !outgoing[&node_id].iter().any(|edge| edge.port == port)) {
                    return Err(WorkflowCompileError::InvalidBranch(format!(
                        "Branch {node_id:?} requires a successor for every case and ELSE"
                    )));
                }
                available_after.insert(node_id.clone(), available_before.clone());
                WorkflowNodeProgram::Branch { cases: cases.clone() }
            }
            WorkflowNodeDefinition::Return { outputs } => {
                if container.is_some() {
                    return Err(WorkflowCompileError::InvalidField(
                        "Return ends the workflow; a body ends where its nodes do".into(),
                    ));
                }
                if !outgoing[&node_id].is_empty() {
                    return Err(WorkflowCompileError::Unterminated(format!(
                        "Return node {node_id:?} cannot have outgoing edges"
                    )));
                }
                let (output, schema) = return_output(outputs, &available_before, &node_id)?;
                returned.push(schema);
                available_after.insert(node_id.clone(), available_before.clone());
                WorkflowNodeProgram::Return { output }
            }
        }) })().map_err(|e: WorkflowCompileError| e.at(format!("nodes.{node_id}")))?;
        let edges = &outgoing[&node_id];
        match &compiled {
            // Return and Branch ports were validated above. A body's path
            // may end at any node.
            WorkflowNodeProgram::Return { .. } | WorkflowNodeProgram::Branch { .. } => {}
            _ if edges.is_empty() && container.is_none() => {
                return Err(WorkflowCompileError::Unterminated(format!(
                    "node {node_id:?} must have at least one successor"
                ))
                .at(format!("nodes.{node_id}")));
            }
            _ => {}
        }
        nodes.insert(node_id, compiled);
    }
    if container.is_none()
        && !nodes
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
        output_schema: merge_return_schemas(&returned)?,
        entry,
        nodes,
        outgoing,
    })
}

struct ControlFlow {
    entry: String,
    scopes: flow::ActivationScopes,
    outgoing: BTreeMap<String, Vec<WorkflowEdgeProgram>>,
    topological: Vec<String>,
}

/// Shared finite control-flow proof for compilation and partial authoring.
#[allow(clippy::too_many_lines)]
fn control_flow(definition: &WorkflowBlock) -> Result<ControlFlow, WorkflowCompileError> {
    if definition.nodes.is_empty() || definition.nodes.len() > MAX_WORKFLOW_NODES {
        return Err(WorkflowCompileError::InvalidField(format!(
            "block must contain between one and {MAX_WORKFLOW_NODES} nodes"
        )));
    }
    let starts = definition
        .nodes
        .iter()
        .filter_map(|(id, node)| matches!(node, WorkflowNodeDefinition::Start {}).then_some(id))
        .collect::<Vec<_>>();
    if starts.len() != 1 {
        return Err(WorkflowCompileError::InvalidEntry(
            "block must contain exactly one Start node".into(),
        )
        .at("nodes"));
    }
    let entry = starts[0].clone();
    for node_id in definition.nodes.keys() {
        if node_id == "args" || !valid_local_key(node_id) {
            return Err(WorkflowCompileError::InvalidField(format!(
                "workflow node id {node_id:?} must be non-empty, at most 64 bytes, and contain no dots"
            )).at(format!("nodes.{node_id}")));
        }
    }
    for (id, node) in &definition.nodes {
        if let WorkflowNodeDefinition::Branch { cases } = node {
            if cases.is_empty() || cases.len() > 32 {
                return Err(WorkflowCompileError::InvalidBranch(
                    "Branch requires 1..=32 cases".into(),
                )
                .at(format!("nodes.{id}.cases")));
            }
            let mut ids = BTreeSet::new();
            for (index, case) in cases.iter().enumerate() {
                if !valid_local_key(&case.id) || is_reserved_port(&case.id) || !ids.insert(&case.id)
                {
                    return Err(WorkflowCompileError::InvalidBranch(
                        "case IDs must be unique valid local keys and cannot name a reserved port"
                            .into(),
                    )
                    .at(format!("nodes.{id}.cases.{index}.id")));
                }
            }
        }
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
    for (edge_index, edge) in definition.edges.iter().enumerate() {
        if !definition.nodes.contains_key(&edge.from) {
            return Err(WorkflowCompileError::DanglingReference(format!(
                "edge source {:?} is not a workflow node",
                edge.from
            ))
            .at(format!("edges.{edge_index}.from")));
        }
        if !definition.nodes.contains_key(&edge.to) {
            return Err(WorkflowCompileError::DanglingReference(format!(
                "edge destination {:?} is not a workflow node",
                edge.to
            ))
            .at(format!("edges.{edge_index}.to")));
        }
        let node = definition.nodes.get(&edge.from).expect("checked above");
        let port = match (node, edge.port.clone()) {
            (WorkflowNodeDefinition::Branch { cases }, Some(port))
                if port == WorkflowPort::False
                    || cases
                        .iter()
                        .any(|case| WorkflowPort::case(&case.id) == port) =>
            {
                port
            }
            (WorkflowNodeDefinition::Branch { .. }, _) => {
                return Err(WorkflowCompileError::InvalidBranch(format!(
                    "Branch node {:?} must use an authored case ID or false (ELSE)",
                    edge.from
                ))
                .at(format!("edges.{edge_index}.port")));
            }
            (
                WorkflowNodeDefinition::Model {
                    on_error: WorkflowModelErrorStrategy::FailBranch,
                    ..
                }
                | WorkflowNodeDefinition::Agent {
                    on_error: WorkflowValueErrorStrategy::FailBranch,
                    ..
                }
                | WorkflowNodeDefinition::Tool {
                    on_error: WorkflowValueErrorStrategy::FailBranch,
                    ..
                },
                Some(WorkflowPort::Fail),
            ) => WorkflowPort::Fail,
            (WorkflowNodeDefinition::Agent { routes, .. }, Some(port))
                if routes
                    .iter()
                    .any(|route| WorkflowPort::case(&route.id) == port) =>
            {
                port
            }
            (WorkflowNodeDefinition::Review { actions, .. }, Some(port))
                if port == WorkflowPort::case(REVIEW_TIMEOUT_PORT)
                    || actions
                        .iter()
                        .any(|action| WorkflowPort::case(&action.id) == port) =>
            {
                port
            }
            (WorkflowNodeDefinition::Review { .. }, _) => {
                return Err(WorkflowCompileError::InvalidBranch(format!(
                    "Human Input node {:?} must use one of its action IDs or timeout",
                    edge.from
                ))
                .at(format!("edges.{edge_index}.port")));
            }
            (WorkflowNodeDefinition::Agent { routes, .. }, _) if !routes.is_empty() => {
                return Err(WorkflowCompileError::InvalidBranch(format!(
                    "routed Agent node {:?} must use one of its route IDs",
                    edge.from
                ))
                .at(format!("edges.{edge_index}.port")));
            }
            (_, None) => WorkflowPort::Next,
            (_, Some(port)) => {
                return Err(WorkflowCompileError::InvalidField(format!(
                    "ordinary node {:?} cannot have port {port:?}",
                    edge.from
                ))
                .at(format!("edges.{edge_index}.port")));
            }
        };
        let successors = outgoing.get_mut(&edge.from).expect("node exists");
        // Validate in authored order before sorting the immutable runtime edges.
        // The later identical connection owns the duplicate-edge diagnostic.
        if successors
            .iter()
            .any(|successor| successor.port == port && successor.to == edge.to)
        {
            return Err(WorkflowCompileError::InvalidBranch(format!(
                "node {:?} already has this {port:?} connection",
                edge.from
            ))
            .at(format!("edges.{edge_index}.port")));
        }
        successors.push(WorkflowEdgeProgram {
            to: edge.to.clone(),
            port,
        });
        *incoming.get_mut(&edge.to).expect("node exists") += 1;
    }
    for edges in outgoing.values_mut() {
        edges.sort_by(|left, right| left.port.cmp(&right.port).then(left.to.cmp(&right.to)));
    }
    // A fail branch is a path of its own, so it must lead somewhere.
    for (id, node) in &definition.nodes {
        if matches!(
            node,
            WorkflowNodeDefinition::Model {
                on_error: WorkflowModelErrorStrategy::FailBranch,
                ..
            } | WorkflowNodeDefinition::Agent {
                on_error: WorkflowValueErrorStrategy::FailBranch,
                ..
            } | WorkflowNodeDefinition::Tool {
                on_error: WorkflowValueErrorStrategy::FailBranch,
                ..
            }
        ) && !outgoing[id]
            .iter()
            .any(|edge| edge.port == WorkflowPort::Fail)
        {
            return Err(WorkflowCompileError::InvalidField(format!(
                "node {id:?} handles errors with a fail branch but has no fail connection"
            ))
            .at(format!("nodes.{id}.on_error")));
        }
    }
    if incoming[&entry] != 0 {
        return Err(WorkflowCompileError::InvalidEntry(format!(
            "entry node {:?} must not have an incoming edge",
            entry
        ))
        .at(format!("nodes.{entry}")));
    }
    let topological = topological_order(&outgoing, &incoming)?;
    let reachable = reachable_nodes(&entry, &outgoing);
    if let Some(unreachable) = definition
        .nodes
        .keys()
        .find(|node| !reachable.contains(*node))
    {
        return Err(WorkflowCompileError::Unreachable(unreachable.clone())
            .at(format!("nodes.{unreachable}")));
    }

    let scopes = flow::ActivationScopes::new(definition, &entry, &outgoing, &topological)?;
    Ok(ControlFlow {
        outgoing,
        topological,
        entry,
        scopes,
    })
}

/// Deserializes a bounded keyed map without allowing a later YAML/JSON key to
/// silently replace an earlier one. Stable node identity
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

/// Native language shape only; the selected Tool's complete contract is separate.
fn validate_tool_arguments_root(schema: &Value) -> Result<(), WorkflowCompileError> {
    if schema_type(schema) != Some("object") {
        return Err(WorkflowCompileError::InvalidField(
            "Tool arguments must construct an object".into(),
        ));
    }
    Ok(())
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
/// `description` and `default` are annotations; a property's `default` is
/// filled in at block entry (see [`apply_schema_defaults`]).
const WORKFLOW_SCHEMA_KEYWORDS: &[&str] = &[
    "additionalProperties",
    "const",
    "default",
    "description",
    "enum",
    "items",
    "maxLength",
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
    if object.get("description").is_some_and(|description| {
        description
            .as_str()
            .is_none_or(|text| text.len() > crate::model::structured::SCHEMA_DESCRIPTION_BYTES_MAX)
    }) {
        return Err(WorkflowCompileError::InvalidSchema(
            "schema description must be a string of at most 4096 UTF-8 bytes".into(),
        )
        .at("description"));
    }
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
        let mut seen = std::collections::HashSet::new();
        if enum_values.is_empty() || enum_values.iter().any(|value| !seen.insert(value)) {
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
    if let Some(max_length) = object.get("maxLength") {
        if kind != "string" {
            return Err(WorkflowCompileError::InvalidSchema(format!(
                "workflow schema {path:?} maxLength is only valid for string schemas"
            )));
        }
        if !max_length
            .as_u64()
            .is_some_and(|max| (1..=MAX_VALUE_BYTES as u64).contains(&max))
        {
            return Err(WorkflowCompileError::InvalidSchema(format!(
                "workflow schema {path:?} maxLength must be an integer from 1 to {MAX_VALUE_BYTES}"
            ))
            .at("maxLength"));
        }
    }
    if let Some(default) = object.get("default") {
        let mut constraint = object.clone();
        constraint.remove("default");
        let valid = jsonschema::Validator::new(&Value::Object(constraint))
            .is_ok_and(|validator| validator.is_valid(default));
        if !valid {
            return Err(WorkflowCompileError::InvalidSchema(format!(
                "workflow schema {path:?} default does not satisfy its own schema"
            ))
            .at("default"));
        }
    }
    for value in object
        .get("const")
        .into_iter()
        .chain(object.get("default"))
        .chain(
            object
                .get("enum")
                .and_then(Value::as_array)
                .into_iter()
                .flatten(),
        )
    {
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
                    )
                    .map_err(|e| e.at(format!("properties.{name}")))?;
                }
            }
            if let Some(required) = object.get("required") {
                let Some(required) = required.as_array() else {
                    return Err(WorkflowCompileError::InvalidSchema(format!(
                        "workflow schema {path:?} required must be an array of property names"
                    )));
                };
                let empty = serde_json::Map::new();
                let properties = object
                    .get("properties")
                    .and_then(Value::as_object)
                    .unwrap_or(&empty);
                let mut seen = std::collections::HashSet::new();
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
                    if !seen.insert(name) {
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
                validate_workflow_schema_at(items, &format!("{path}.items"), depth + 1)
                    .map_err(|e| e.at("items"))?;
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
    task: &[WorkflowValue],
    output: &WorkflowAgentOutput,
    routes: &[WorkflowAgentRoute],
    on_error: &WorkflowValueErrorStrategy,
    available: &SchemaMap,
    node: &str,
) -> Result<(), WorkflowCompileError> {
    if validate_prompt(
        task,
        available,
        node,
        &format!("Agent {node:?} task"),
        "task",
    )? {
        return Err(
            WorkflowCompileError::InvalidField(format!("Agent {node:?} task is empty")).at("task"),
        );
    }
    if let Some(schema) = output.structured() {
        validate_root_schema(schema, &format!("Agent {node} output"))
            .map_err(|e| e.at("output.schema"))?;
        if !routes.is_empty()
            && schema
                .pointer(&format!("/properties/{AGENT_ROUTE_FIELD}"))
                .is_some()
        {
            return Err(WorkflowCompileError::InvalidSchema(format!(
                "Agent {node:?} routes reserve the {AGENT_ROUTE_FIELD:?} output field"
            ))
            .at("output.schema"));
        }
    }
    if routes.len() > MAX_AGENT_ROUTES {
        return Err(WorkflowCompileError::InvalidBranch(format!(
            "Agent {node:?} has at most {MAX_AGENT_ROUTES} routes"
        ))
        .at("routes"));
    }
    let mut ids = BTreeSet::new();
    for (index, route) in routes.iter().enumerate() {
        if !valid_local_key(&route.id) || is_reserved_port(&route.id) || !ids.insert(&route.id) {
            return Err(WorkflowCompileError::InvalidBranch(
                "route IDs must be unique valid local keys and cannot name a reserved port".into(),
            )
            .at(format!("routes.{index}.id")));
        }
        if route.description.trim().is_empty()
            || route.description.len() > MAX_AGENT_ROUTE_DESCRIPTION_BYTES
        {
            return Err(WorkflowCompileError::InvalidField(format!(
                "Agent {node:?} route descriptions tell the child when to choose them, within {MAX_AGENT_ROUTE_DESCRIPTION_BYTES} bytes"
            ))
            .at(format!("routes.{index}.description")));
        }
    }
    if let WorkflowValueErrorStrategy::DefaultValue { value } = on_error {
        let schema = agent_value_schema(output, routes);
        if !jsonschema::Validator::new(&schema).is_ok_and(|validator| validator.is_valid(value)) {
            return Err(WorkflowCompileError::InvalidField(format!(
                "Agent {node:?} default value must satisfy the node's own value contract"
            ))
            .at("on_error.value"));
        }
    }
    Ok(())
}

/// Ports a Branch case or an Agent route cannot be named after.
/// As Dify's End: each output variable a unique field read from a variable.
/// The value is the object those fields make, and its schema.
fn return_output(
    outputs: &[WorkflowReturnOutput],
    available: &SchemaMap,
    node: &str,
) -> Result<(WorkflowValue, Value), WorkflowCompileError> {
    let (mut fields, mut properties) = (BTreeMap::new(), serde_json::Map::new());
    for (index, output) in outputs.iter().enumerate() {
        if !valid_local_key(&output.variable) || fields.contains_key(&output.variable) {
            return Err(WorkflowCompileError::InvalidField(
                "output variables must be unique valid local keys".into(),
            )
            .at(format!("outputs.{index}.variable")));
        }
        let value = WorkflowValue::Reference {
            path: output.value_selector.clone(),
        };
        let schema = value_schema(&value, available, node, 0)
            .map_err(|e| e.at(format!("outputs.{index}.value_selector")))?;
        properties.insert(output.variable.clone(), schema);
        fields.insert(output.variable.clone(), value);
    }
    let required = fields.keys().cloned().collect::<Vec<_>>();
    Ok((
        WorkflowValue::Object { fields },
        serde_json::json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false}),
    ))
}

/// The workflow's output: every Return's fields, required when every Return
/// has it. A field two Returns share must have one type.
fn merge_return_schemas(returned: &[Value]) -> Result<Value, WorkflowCompileError> {
    let Some((first, rest)) = returned.split_first() else {
        return Ok(empty_object_schema());
    };
    let mut merged = first.clone();
    for schema in rest {
        for (key, property) in schema["properties"].as_object().into_iter().flatten() {
            let properties = merged["properties"].as_object_mut().expect("object schema");
            // The wider of the two, when one admits every value of the other.
            match properties.get(key) {
                Some(existing) if schemas_compatible(property, existing) => {}
                Some(existing) if !schemas_compatible(existing, property) => {
                    return Err(WorkflowCompileError::IncompatibleReference(format!(
                        "Return nodes give output {key:?} different types"
                    )));
                }
                _ => {
                    properties.insert(key.clone(), property.clone());
                }
            }
        }
        let required = merged["required"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|key| {
                schema["required"]
                    .as_array()
                    .is_some_and(|keys| keys.contains(key))
            })
            .cloned()
            .collect::<Vec<_>>();
        merged["required"] = Value::Array(required);
    }
    Ok(merged)
}

fn validate_review(
    content: &[WorkflowValue],
    actions: &[WorkflowReviewAction],
    timeout_ms: u64,
    available: &SchemaMap,
    node: &str,
) -> Result<(), WorkflowCompileError> {
    if validate_prompt(
        content,
        available,
        node,
        &format!("Human Input {node:?} content"),
        "content",
    )? {
        return Err(WorkflowCompileError::InvalidField(format!(
            "Human Input {node:?} content is empty"
        ))
        .at("content"));
    }
    if actions.is_empty() || actions.len() > MAX_REVIEW_ACTIONS {
        return Err(WorkflowCompileError::InvalidBranch(format!(
            "Human Input {node:?} has 1..={MAX_REVIEW_ACTIONS} actions"
        ))
        .at("actions"));
    }
    let (mut ids, mut titles) = (BTreeSet::new(), BTreeSet::new());
    for (index, action) in actions.iter().enumerate() {
        if !valid_local_key(&action.id)
            || is_reserved_port(&action.id)
            || action.id == REVIEW_TIMEOUT_PORT
            || !ids.insert(&action.id)
        {
            return Err(WorkflowCompileError::InvalidBranch(
                "action IDs must be unique valid local keys and cannot name a reserved port".into(),
            )
            .at(format!("actions.{index}.id")));
        }
        let title = action.title.trim();
        if title.is_empty()
            || action.title.chars().count() > crate::events::interaction::MAX_OPTION_LABEL_CHARS
            || !titles.insert(title)
        {
            return Err(WorkflowCompileError::InvalidField(format!(
                "action titles must be unique non-empty text of at most {} characters",
                crate::events::interaction::MAX_OPTION_LABEL_CHARS
            ))
            .at(format!("actions.{index}.title")));
        }
    }
    if timeout_ms == 0 || timeout_ms > 86_400_000 {
        return Err(
            WorkflowCompileError::InvalidField("timeout_ms must be 1..=86400000".into())
                .at("timeout_ms"),
        );
    }
    Ok(())
}

fn is_reserved_port(id: &str) -> bool {
    matches!(id, "false" | "next" | "fail")
}

/// Check one prompt's parts, fixed text or references, against its bounds and
/// the available scope. Returns whether it has no text and no variable.
fn validate_prompt(
    parts: &[WorkflowValue],
    available: &SchemaMap,
    node: &str,
    what: &str,
    at: &str,
) -> Result<bool, WorkflowCompileError> {
    if parts.len() > MAX_MODEL_MESSAGE_PARTS {
        return Err(WorkflowCompileError::InvalidField(format!(
            "{what} exceeds {MAX_MODEL_MESSAGE_PARTS} parts"
        ))
        .at(at));
    }
    let mut text = 0usize;
    for (position, part) in parts.iter().enumerate() {
        match part {
            WorkflowValue::Literal {
                value: Value::String(fixed),
            } => text += fixed.len(),
            WorkflowValue::Reference { .. } => {
                value_schema(part, available, node, 0)
                    .map_err(|e| e.at(format!("{at}.{position}")))?;
            }
            _ => {
                return Err(WorkflowCompileError::InvalidField(format!(
                    "{what} content is text or references only"
                ))
                .at(format!("{at}.{position}")));
            }
        }
    }
    if text > MAX_MODEL_MESSAGE_BYTES {
        return Err(WorkflowCompileError::InvalidField(format!(
            "{what} text exceeds {MAX_MODEL_MESSAGE_BYTES} bytes"
        ))
        .at(at));
    }
    Ok(parts.iter().all(|part| {
        matches!(part, WorkflowValue::Literal { value: Value::String(fixed) } if fixed.trim().is_empty())
    }))
}

fn validate_model(
    model: &crate::model::session::AuxiliaryModelPolicy,
    messages: &[WorkflowModelMessage],
    output: &WorkflowModelOutput,
    available: &SchemaMap,
    node: &str,
) -> Result<(), WorkflowCompileError> {
    validate_model_policy(model).map_err(|error| error.at("model"))?;
    if messages.is_empty() || messages.len() > MAX_MODEL_MESSAGES {
        return Err(WorkflowCompileError::InvalidField(format!(
            "Model {node:?} needs a system message and at most {MAX_MODEL_MESSAGES} messages"
        ))
        .at("messages"));
    }
    for (index, message) in messages.iter().enumerate() {
        let at = format!("messages.{index}");
        if (index == 0) != (message.role == WorkflowModelRole::System) {
            return Err(WorkflowCompileError::InvalidField(format!(
                "Model {node:?} message {index} must be {}",
                if index == 0 {
                    "the system message"
                } else {
                    "a user or assistant message"
                }
            ))
            .at(format!("{at}.role")));
        }
        let empty = validate_prompt(
            &message.content,
            available,
            node,
            &format!("Model {node:?} message {index}"),
            &format!("{at}.content"),
        )?;
        if index > 0 && empty {
            return Err(WorkflowCompileError::InvalidField(format!(
                "Model {node:?} message {index} is empty"
            ))
            .at(format!("{at}.content")));
        }
    }
    if !messages
        .iter()
        .any(|message| message.role == WorkflowModelRole::User)
    {
        return Err(WorkflowCompileError::InvalidField(format!(
            "Model {node:?} needs at least one user message"
        ))
        .at("messages"));
    }
    validate_model_output(output, node)
}

fn validate_retry(
    retry: Option<&WorkflowRetry>,
    kind: &str,
    node: &str,
) -> Result<(), WorkflowCompileError> {
    if let Some(retry) = retry
        && (!(1..=MAX_RETRIES).contains(&retry.max_retries)
            || !RETRY_INTERVAL_MS.contains(&retry.interval_ms))
    {
        return Err(WorkflowCompileError::InvalidField(format!(
            "{kind} {node:?} retries 1..={MAX_RETRIES} times, 100..=5000 ms apart"
        ))
        .at("retry"));
    }
    Ok(())
}

fn validate_model_failure(
    retry: Option<&WorkflowRetry>,
    on_error: &WorkflowModelErrorStrategy,
    output: &WorkflowModelOutput,
    node: &str,
) -> Result<(), WorkflowCompileError> {
    validate_retry(retry, "Model", node)?;
    let WorkflowModelErrorStrategy::DefaultValue {
        text,
        structured_output,
    } = on_error
    else {
        return Ok(());
    };
    if text.len() > MAX_MODEL_MESSAGE_BYTES {
        return Err(WorkflowCompileError::InvalidField(format!(
            "Model {node:?} default text exceeds {MAX_MODEL_MESSAGE_BYTES} bytes"
        ))
        .at("on_error.text"));
    }
    match (output, structured_output) {
        (WorkflowModelOutput::Text {}, None) => Ok(()),
        (WorkflowModelOutput::JsonSchema { schema }, Some(value))
            if jsonschema::Validator::new(schema).is_ok_and(|validator| validator.is_valid(value)) =>
        {
            Ok(())
        }
        _ => Err(WorkflowCompileError::InvalidField(format!(
            "Model {node:?} default structured_output must satisfy its declared schema, and only a declared schema has one"
        ))
        .at("on_error.structured_output")),
    }
}

/// Shared output-contract proof, independent of unfinished messages.
fn validate_model_output(
    output: &WorkflowModelOutput,
    node: &str,
) -> Result<(), WorkflowCompileError> {
    if let WorkflowModelOutput::JsonSchema { schema } = output {
        expressions::bounded_value_bytes(schema).map_err(|_| {
            WorkflowCompileError::InvalidSchema(
                "Model output schema exceeds its byte/depth bound".into(),
            )
            .at("output.schema")
        })?;
        validate_root_schema(schema, &format!("Model {node} output"))
            .map_err(|e| e.at("output.schema"))?;
        crate::model::structured::ResponseSchema::new(
            "workflow_model".into(),
            schema.clone(),
            MAX_VALUE_BYTES,
        )
        .map_err(|error| WorkflowCompileError::InvalidSchema(error.message).at("output.schema"))?;
    }
    Ok(())
}

/// Structural policy validity is independent of model resources or credentials.
/// Protocol-specific ownership, Profile defaults and output maxima are checked
/// by the existing model resolver against the invoking Attempt's registry.
fn validate_model_policy(
    policy: &crate::model::session::AuxiliaryModelPolicy,
) -> Result<(), WorkflowCompileError> {
    use crate::model::session::AuxiliaryModelPolicy;
    let AuxiliaryModelPolicy::Explicit {
        model,
        profile,
        request_params,
        max_output_tokens,
    } = policy
    else {
        return Ok(());
    };
    if model.as_str().len() > MAX_RUN_ID_COMPONENT_BYTES
        || crate::model::catalog::ModelRef::parse(model.as_str()).is_err()
    {
        return Err(WorkflowCompileError::InvalidField(
            "Model selection has an invalid or oversized identity".into(),
        )
        .at("model"));
    }
    if let Some(profile) = profile
        && (profile.as_str().len() > MAX_RUN_ID_COMPONENT_BYTES
            || crate::model::catalog::ModelProfileId::parse(profile.as_str()).is_err())
    {
        return Err(WorkflowCompileError::InvalidField(
            "Model Profile has an invalid or oversized identity".into(),
        )
        .at("profile"));
    }
    expressions::bounded_value_bytes(&Value::Object(request_params.clone())).map_err(|_| {
        WorkflowCompileError::InvalidField(
            "Model request parameters exceed their byte/depth bound".into(),
        )
        .at("request_params")
    })?;
    crate::model::invocation::validate_request_params_numbers(
        request_params,
        crate::model::invocation::RequestParamsLayer::SessionOverrides,
    )
    .map_err(|error| WorkflowCompileError::InvalidField(error.to_string()).at("request_params"))?;
    if *max_output_tokens == Some(0) {
        return Err(WorkflowCompileError::InvalidField(
            "Model max_output_tokens must be positive".into(),
        )
        .at("max_output_tokens"));
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
        // BTreeMap order selects the lexicographically smallest Kahn residual,
        // independent of authored edge order. It may be downstream of a cycle.
        let node = counts
            .iter()
            .find_map(|(node, count)| (*count > 0).then_some(node.clone()))
            .expect("incomplete topological order has residual nodes");
        return Err(WorkflowCompileError::Cycle { node: node.clone() }.at(format!("nodes.{node}")));
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
    /// A block's entry environment: `args` after defaults are filled in.
    fn from_schema(schema: &Value) -> Self {
        Self(BTreeMap::from([(
            "args".into(),
            schema_with_defaults_required(schema),
        )]))
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

/// The schema of a block input after [`apply_schema_defaults`]: every
/// property with a `default` is present, so it is required.
fn schema_with_defaults_required(schema: &Value) -> Value {
    let Some(object) = schema.as_object() else {
        return schema.clone();
    };
    let mut result = object.clone();
    if let Some(Value::Object(properties)) = result.get_mut("properties") {
        let defaulted = properties
            .iter()
            .filter(|(_, property)| property.get("default").is_some())
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        for property in properties.values_mut() {
            *property = schema_with_defaults_required(property);
        }
        if !defaulted.is_empty() {
            let mut required = schema_required(schema)
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            for name in defaulted {
                if !required.contains(&name) {
                    required.push(name);
                }
            }
            result.insert(
                "required".into(),
                Value::Array(required.into_iter().map(Value::String).collect()),
            );
        }
    }
    if let Some(items) = result.get_mut("items") {
        *items = schema_with_defaults_required(items);
    }
    Value::Object(result)
}

/// Fill every absent property that declares a `default`, recursively through
/// present objects and array items.
pub(crate) fn apply_schema_defaults(schema: &Value, value: &mut Value) {
    match value {
        Value::Object(fields) => {
            let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
                return;
            };
            for (name, property) in properties {
                match fields.get_mut(name) {
                    Some(field) => apply_schema_defaults(property, field),
                    None => {
                        if let Some(default) = property.get("default") {
                            fields.insert(name.clone(), default.clone());
                        }
                    }
                }
            }
        }
        Value::Array(items) => {
            if let Some(item_schema) = schema.get("items") {
                for item in items {
                    apply_schema_defaults(item_schema, item);
                }
            }
        }
        _ => {}
    }
}

fn finite_schema_values(schema: &Value) -> Option<&[Value]> {
    if let Some(constant) = schema.get("const") {
        if let Some(Value::Array(enum_values)) = schema.get("enum") {
            return enum_values
                .contains(constant)
                .then_some(std::slice::from_ref(constant));
        }
        return Some(std::slice::from_ref(constant));
    }
    schema
        .get("enum")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
}

fn schema_forbids_property(schema: &Value, key: &str) -> bool {
    schema_type(schema) == Some("object")
        && schema.get("additionalProperties") == Some(&Value::Bool(false))
        && schema
            .get("properties")
            .and_then(|properties| properties.get(key))
            .is_none()
}

#[derive(Clone, Copy)]
enum SchemaProofStep {
    Structure,
    CompileValidator,
    ValidateValue,
}

fn schemas_compatible(actual: &Value, expected: &Value) -> bool {
    schemas_compatible_with(actual, expected, &mut |_, _, _| {
        Ok::<(), std::convert::Infallible>(())
    })
    .unwrap_or_else(|error| match error {})
}

/// Every recursive proof and finite-value validation is admitted before work.
/// Compilation supplies an unlimited policy; authoring uses its request budget.
#[allow(clippy::too_many_lines)]
fn schemas_compatible_with<E>(
    actual: &Value,
    expected: &Value,
    admit: &mut impl FnMut(&Value, &Value, SchemaProofStep) -> Result<(), E>,
) -> Result<bool, E> {
    admit(actual, expected, SchemaProofStep::Structure)?;
    if let Some(values) = finite_schema_values(actual) {
        if values.is_empty() {
            return Ok(false);
        }
        admit(actual, expected, SchemaProofStep::CompileValidator)?;
        let Ok(expected_validator) = jsonschema::Validator::new(expected) else {
            return Ok(false);
        };
        for value in values {
            admit(value, expected, SchemaProofStep::ValidateValue)?;
            if !expected_validator.is_valid(value) {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    if finite_schema_values(expected).is_some() {
        return Ok(false);
    }
    let Some(actual_type) = schema_type(actual) else {
        return Ok(false);
    };
    let Some(expected_type) = schema_type(expected) else {
        return Ok(false);
    };
    if actual_type != expected_type && !(actual_type == "integer" && expected_type == "number") {
        return Ok(false);
    }
    if let Some(expected_max) = expected.get("maxLength").and_then(Value::as_u64)
        && actual
            .get("maxLength")
            .and_then(Value::as_u64)
            .is_none_or(|actual_max| actual_max > expected_max)
    {
        return Ok(false);
    }
    if expected_type == "object" {
        let empty = serde_json::Map::new();
        let expected_properties = expected
            .get("properties")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        let actual_properties = actual
            .get("properties")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        let expected_required = schema_required(expected);
        let actual_required = schema_required(actual);
        for key in &expected_required {
            if !actual_properties.contains_key(*key)
                || !expected_properties.contains_key(*key)
                || !actual_required.contains(key)
            {
                return Ok(false);
            }
        }
        // Required members recurse here exactly once, together with optional
        // declared members; a required-property chain must not grow as 2^depth.
        for (key, expected_property) in expected_properties {
            match actual_properties.get(key) {
                Some(actual_property) => {
                    if !schemas_compatible_with(actual_property, expected_property, admit)? {
                        return Ok(false);
                    }
                }
                None if !actual
                    .get("additionalProperties")
                    .is_some_and(|value| value == &Value::Bool(false)) =>
                {
                    return Ok(false);
                }
                None => {}
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
                    .any(|key| schema_forbids_property(expected, key)))
        {
            return Ok(false);
        }
    }
    if expected_type == "array" {
        match (actual.get("items"), expected.get("items")) {
            (Some(actual_items), Some(expected_items)) => {
                if !schemas_compatible_with(actual_items, expected_items, admit)? {
                    return Ok(false);
                }
            }
            (None, Some(_)) => return Ok(false),
            _ => {}
        }
    }
    Ok(true)
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

/// How one Workflow-owned child `AgentRun` settles: its final reply is the
/// result, or its committed `workflow_output` value is.
#[derive(Clone)]
pub(crate) enum WorkflowChildTerminal {
    Text,
    Output(Arc<WorkflowOutputLatch>),
}

impl WorkflowChildTerminal {
    pub(crate) fn latch(&self) -> Option<&Arc<WorkflowOutputLatch>> {
        match self {
            Self::Text => None,
            Self::Output(latch) => Some(latch),
        }
    }
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
    program: Arc<WorkflowProgram>,
    model_invocations: BTreeMap<Vec<String>, crate::model::ResolvedModelInvocation>,
    run_id: WorkflowRunId,
    budgets: std::sync::Mutex<execution::RunBudgets>,
    effects: Option<Arc<scheduled::effects::ScheduledEffects>>,
    terminal: Option<WorkflowTerminalState>,
    tools: BTreeMap<
        crate::capabilities::selection::ExactToolSelector,
        crate::tools::types::ToolDefinition,
    >,
}

impl fmt::Debug for WorkflowRun {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkflowRun")
            .field("program", &self.program.id())
            .field("run_id", &self.run_id)
            .field("budgets", &self.budgets)
            .field("terminal", &self.terminal)
            .field("tools", &self.tools)
            .finish_non_exhaustive()
    }
}

impl WorkflowRun {
    fn new(program: Arc<WorkflowProgram>, run_id: WorkflowRunId) -> Self {
        let budgets = execution::RunBudgets::reserved(program.retained_bound);
        Self {
            program,
            model_invocations: BTreeMap::new(),
            run_id,
            budgets: std::sync::Mutex::new(budgets),
            effects: None,
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
    read_model: read_model::WorkflowReadModel,
    subagents: crate::runtime::subagent::SubagentRegistry,
    /// The existing conversation Event Journal. Workflow lifecycle facts are
    /// best-effort observability here; the journal never becomes the
    /// `WorkflowRun` state authority.
    event_store: Arc<dyn ConversationStore>,
    next_run: Arc<std::sync::atomic::AtomicU64>,
    #[cfg(test)]
    node_frontier: Arc<std::sync::Mutex<Option<execution::NodeFrontierHook>>>,
    #[cfg(test)]
    iteration_frontier: Arc<std::sync::Mutex<Option<execution::IterationFrontierHook>>>,
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
        read_model: read_model::WorkflowReadModel,
    ) -> Self {
        Self {
            read_model,
            subagents,
            event_store,
            next_run: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            #[cfg(test)]
            node_frontier: Arc::new(std::sync::Mutex::new(None)),
            #[cfg(test)]
            iteration_frontier: Arc::new(std::sync::Mutex::new(None)),
            #[cfg(test)]
            observations: tokio::sync::watch::Sender::new(Vec::new()),
        }
    }

    #[cfg(test)]
    async fn run_test_foreground(
        &self,
        program: Arc<WorkflowProgram>,
        run_id: ToolCallId,
        context: crate::runtime::subagent::NativeExecutionContext,
        input: Value,
        cancellation: crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<Value, WorkflowRunError> {
        let context = context.with_test_workflow(&program);
        self.run_foreground(program.id().clone(), run_id, context, input, cancellation)
            .await
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
    #[allow(clippy::too_many_lines)] // One admission and terminal settlement protocol.
    pub async fn run_foreground(
        &self,
        workflow: WorkflowId,
        run_id: ToolCallId,
        context: crate::runtime::subagent::NativeExecutionContext,
        input: Value,
        cancellation: crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<Value, WorkflowRunError> {
        let program = context
            .resources()
            .workflows()
            .executable(&workflow)?
            .clone();
        let attempt_id = context.attempt_id().ok_or_else(|| {
            WorkflowRunError::InvocationAuthority(
                "foreground Workflow requires a Tool Attempt".into(),
            )
        })?;
        if attempt_id.as_str().is_empty() || attempt_id.as_str().len() > MAX_RUN_ID_COMPONENT_BYTES
        {
            return Err(WorkflowRunError::InvalidInput(
                "native run identity exceeds its bound".into(),
            ));
        }
        let invocation = self
            .next_run
            .fetch_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |n| n.checked_add(1),
            )
            .map_err(|_| {
                WorkflowRunError::InvalidProgram("Workflow invocation identity exhausted".into())
            })?;
        let native_id = WorkflowRunId {
            conversation_id: self.event_store.conversation_id().clone(),
            origin: attempt_id.clone().into(),
            invocation,
        };
        self.run_native(
            program,
            native_id,
            Some(run_id),
            context,
            input,
            cancellation,
            None,
        )
        .await
    }

    pub(crate) async fn run_scheduled(
        &self,
        claim: scheduled::lifecycle::ClaimedScheduledWorkflow,
        prepared: scheduled::PreparedScheduledWorkflow,
        context: crate::runtime::subagent::NativeExecutionContext,
        cancellation: crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<Value, WorkflowRunError> {
        if context.origin() != &claim.start.head.run_id.origin
            || prepared.contract().binding != claim.start.head.binding
        {
            return Err(WorkflowRunError::InvocationAuthority(
                "scheduled origin or binding mismatch".into(),
            ));
        }
        let input =
            claim.start.arguments.clone().ok_or_else(|| {
                WorkflowRunError::InvalidInput("scheduled arguments missing".into())
            })?;
        let run_id = claim.start.head.run_id.clone();
        let tracker = std::sync::Arc::new(scheduled::effects::ScheduledEffects::new(
            self.event_store.clone(),
            claim,
        ));
        self.run_native(
            prepared.program().clone(),
            run_id,
            None,
            context,
            input,
            cancellation,
            Some(tracker),
        )
        .await
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn run_native(
        &self,
        program: Arc<WorkflowProgram>,
        native_id: WorkflowRunId,
        tool_call_id: Option<ToolCallId>,
        context: crate::runtime::subagent::NativeExecutionContext,
        input: Value,
        cancellation: crate::runtime::cancellation::ExecutionCancellation,
        effects: Option<Arc<scheduled::effects::ScheduledEffects>>,
    ) -> Result<Value, WorkflowRunError> {
        #[cfg(test)]
        crate::local_runtime::static_effects::observe(
            crate::local_runtime::static_effects::Effect::WorkflowRun,
        );
        let model_invocations = model::freeze_models(&program.block, &context)?;
        let mut run = WorkflowRun::new(program.clone(), native_id);
        run.effects = effects;
        run.model_invocations = model_invocations;
        self.read_model.register(read_model::WorkflowRunView {
            id: run.run_id.clone(),
            workflow_id: program.id.clone(),
            program_digest: program.digest.clone(),
            resource_revision: context.resources().revision(),
            tool_call_id: tool_call_id.clone(),
            state: read_model::WorkflowState::Running,
            instances: Vec::new(),
            omitted_instances: 0,
            steps_consumed: 0,
            steps_max: program.execution_bound,
            agents_consumed: 0,
            model_calls_consumed: 0,
        });
        self.emit_observability(
            &run,
            RuntimeEvent::WorkflowStarted {
                tool_call_id,
                workflow_id: program.id().clone(),
                run_id: run.run_id.clone(),
            },
        );
        let execution = {
            let id = run.run_id.clone();
            let execute = async {
                run.tools = program.frozen_tools.clone();
                // The run executes against the invoking Session's execution
                // binding: there is no run-owned workspace to acquire, borrow
                // or settle. Every node settles its own native work inline.
                let execution =
                    match execution::validate_commit(&program.block.input_schema, &input) {
                        Ok(()) => self
                            .execute_block(
                                &run,
                                &program.block,
                                &context,
                                input,
                                vec![0],
                                &cancellation,
                            )
                            .await
                            .map(|output| output.value),
                        Err(error) => Err(error),
                    };
                self.read_model.update(&run.run_id, |view| {
                    if !matches!(view.state, read_model::WorkflowState::Draining) {
                        view.state = read_model::WorkflowState::Waiting {
                            reason: read_model::WorkflowWait::Settlement,
                        };
                    }
                });
                execution
            };
            tokio::pin!(execute);
            tokio::select! {
                biased;
                () = cancellation.cancelled() => {
                    self.read_model.draining(&id);
                    execute.await
                }
                result = &mut execute => result,
            }
        };
        // Run terminal frontier. The shared block executor has already
        // validated its result and settled all owned native work. No await
        // separates this cancellation observation from the unique run commit.
        context.check_workflow_deadline(&cancellation);
        let execution = match execution {
            Ok(_) if cancellation.is_cancelled() => {
                Err(WorkflowRunError::from_cancellation(&cancellation))
            }
            result => result,
        };
        match execution {
            Ok(value) => {
                run.settle(WorkflowTerminalState::Completed(value.clone()))?;
                self.read_model.update(&run.run_id, |view| {
                    view.state = read_model::WorkflowState::Settled {
                        outcome: WorkflowExecutionOutcome::Completed,
                    }
                });
                self.emit_observability(
                    &run,
                    RuntimeEvent::WorkflowCompleted {
                        workflow_id: program.id().clone(),
                        run_id: run.run_id.clone(),
                    },
                );
                // The semantic output is the validated output of all active Returns.
                Ok(value)
            }
            Err(error) => {
                let terminal = match &error {
                    WorkflowRunError::Cancelled(reason) => {
                        WorkflowTerminalState::Cancelled(*reason)
                    }
                    _ => WorkflowTerminalState::Failed(error.clone()),
                };
                run.settle(terminal)?;
                self.read_model.update(&run.run_id, |view| {
                    view.state = read_model::WorkflowState::Settled {
                        outcome: execution::outcome::<()>(&Err(error.clone())),
                    }
                });
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
                Err(error)
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
            tool_occurrence: None,
            observation: None,
            schema_version: EVENT_SCHEMA_VERSION,
            event_id,
            sequence: 0,
            conversation_id: self.event_store.conversation_id().clone(),
            attempt_id: None,
            turn_id: None,
            timestamp: Utc::now(),
            event,
        };
        if let Ok(committed) = self.event_store.append_event(envelope) {
            // Every committed native progress fact releases its preceding
            // read-model revisions. Waiting for the outer run's terminal fact
            // would hide in-flight child/check state behind its own execution.
            self.read_model.publish_committed(committed.sequence);
        }
    }

    #[allow(clippy::too_many_arguments)] // the explicit child admission boundary
    #[allow(clippy::too_many_lines)] // the single native child admission path
    async fn admit_agent(
        &self,
        run: &WorkflowRun,
        context: &crate::runtime::subagent::NativeExecutionContext,
        input: &Value,
        values: &imbl::OrdMap<String, Value>,
        node_id: &WorkflowNodeInstance,
        agent: &WorkflowAgentProgram,
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<crate::runtime::identity::SubagentId, WorkflowRunError> {
        let resolved = context
            .bind_workflow_agent(
                agent
                    .resolved
                    .as_ref()
                    .expect("admission froze every Agent"),
            )
            .map_err(|error| WorkflowRunError::ChildStart {
                node: node_id.to_string(),
                detail: bound_workflow_diagnostic(error.to_string()),
            })?;
        let task = model::render_parts(&agent.task, |part| evaluate_value(part, input, values))?;
        let spec = crate::runtime::subagent::SubagentStartSpec {
            authority: crate::runtime::subagent::DurableAgentAuthority {
                title: node_id.node.clone().into(),
                execution_policy: context.execution_policy(),
                resolved,
                approval_mode: context.approval_mode(),
            },
            admission: crate::runtime::subagent::ActivationAdmission {
                attachments: Vec::new(),
                task,
                context: None,
                origin: crate::runtime::subagent::AgentActivationOrigin::Workflow {
                    node_id: Box::new(node_id.clone()),
                },
                terminal: crate::runtime::subagent::SubagentTerminalMode::WorkflowOutput {
                    output_schema: agent.terminal_schema(),
                    workflow_id: run.program.id().clone(),
                    run_id: run.run_id.clone(),
                    node_id: Box::new(node_id.clone()),
                },
            },
        };
        let child_cancellation = cancellation.child_signal();
        self.read_model.node(node_id, |view| {
            view.state = read_model::WorkflowState::Waiting {
                reason: read_model::WorkflowWait::Capacity,
            }
        });
        // The finite Workflow-owned AgentRun shares the native staged child
        // supervision path and the invoking Session binding. Its terminal mode
        // keeps it out of the ordinary durable Agent resume and mailbox API.
        let prepared = self
            .subagents
            .prepare(&spec, &child_cancellation)
            .await
            .map_err(|error| {
                if !error.rollback_is_proven()
                    && let Some(effects) = &run.effects
                {
                    effects.unproven(node_id, "native Agent rollback has no physical proof");
                }
                match error {
                    crate::runtime::subagent::SubagentStartError::Cancelled => {
                        WorkflowRunError::from_cancellation(cancellation)
                    }
                    error => WorkflowRunError::ChildStart {
                        node: node_id.to_string(),
                        detail: bound_workflow_diagnostic(error.to_string()),
                    },
                }
            })?;
        let accepted = self
            .subagents
            .commit_waiting(prepared, &child_cancellation)
            .await
            .map_err(|error| {
                if !error.rollback_is_proven()
                    && let Some(effects) = &run.effects
                {
                    effects.unproven(node_id, "native Agent rollback has no physical proof");
                }
                match error {
                    crate::runtime::subagent::SubagentStartError::Cancelled => {
                        WorkflowRunError::from_cancellation(cancellation)
                    }
                    error => WorkflowRunError::ChildStart {
                        node: node_id.to_string(),
                        detail: bound_workflow_diagnostic(error.to_string()),
                    },
                }
            })?;
        let crate::runtime::subagent::SubagentStartOutcome::Accepted(accepted) = accepted else {
            return Err(WorkflowRunError::from_cancellation(cancellation));
        };
        if let Some(effects) = &run.effects
            && let Err(error) = effects.owner(
                node_id,
                scheduled::lifecycle::ScheduledEffectOwner::Agent {
                    activation: accepted.subagent_id.clone(),
                },
            )
        {
            let _ = self.subagents.cancel(
                &accepted.subagent_id,
                crate::runtime::types::CancellationReason::ParentCancelled,
            );
            let settled = self
                .subagents
                .wait_until_settled(&accepted.subagent_id)
                .await;
            if !settled.is_some_and(|snapshot| snapshot.is_settled()) {
                effects.unproven(
                    node_id,
                    "Agent owner association failed and native cleanup remains unproven",
                );
            }
            return Err(error);
        }
        self.read_model.node(node_id, |view| {
            view.child = Some(accepted.subagent_id.clone());
            view.child_agent_id = Some(accepted.child_agent_id.clone());
            view.state = read_model::WorkflowState::Waiting {
                reason: read_model::WorkflowWait::Agent,
            };
        });
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
        run: &WorkflowRun,
        subagent_id: crate::runtime::identity::SubagentId,
        node_id: &WorkflowNodeInstance,
        agent: &WorkflowAgentProgram,
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<Value, WorkflowRunError> {
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
                if let Some(snapshot) = snapshot {
                    return self.settled_agent_value(
                            run, snapshot,
                            &subagent_id,
                            node_id,
                            agent,
                            cancellation,
                        );
                }
                return Err(WorkflowRunError::from_cancellation(cancellation));
            }
            snapshot = &mut wait => snapshot,
        };
        let snapshot = snapshot.ok_or_else(|| WorkflowRunError::ChildOutcome {
            node: node_id.to_string(),
            status: crate::tools::types::ToolExecutionStatus::OutcomeUnknown {
                detail: "the native SubagentRegistry lost the child record".to_owned(),
            },
        })?;
        self.settled_agent_value(run, snapshot, &subagent_id, node_id, agent, cancellation)
    }

    fn settled_agent_value(
        &self,
        run: &WorkflowRun,
        snapshot: crate::runtime::subagent::SubagentSnapshot,
        subagent_id: &crate::runtime::identity::SubagentId,
        node_id: &WorkflowNodeInstance,
        agent: &WorkflowAgentProgram,
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<Value, WorkflowRunError> {
        use crate::runtime::subagent::{SubagentPhysicalSettlement, SubagentPublication};
        use crate::tools::types::{ToolCancellationPhase, ToolExecutionStatus as Status};
        if !snapshot.is_settled()
            && let Some(effects) = &run.effects
        {
            effects.unproven(
                node_id,
                "native Agent publication or physical proof is pending",
            );
        }
        let settlement_failure = match snapshot.settlement.publication {
            SubagentPublication::Pending => {
                Some(WorkflowChildSettlementFailure::PublicationPending)
            }
            SubagentPublication::Abandoned => {
                Some(WorkflowChildSettlementFailure::PublicationAbandoned)
            }
            SubagentPublication::Committed if !snapshot.state.is_terminal() => {
                Some(WorkflowChildSettlementFailure::LogicalTerminalMissing)
            }
            SubagentPublication::Committed
                if snapshot.settlement.physical == SubagentPhysicalSettlement::Unproven =>
            {
                Some(WorkflowChildSettlementFailure::PhysicalContainmentUnproven)
            }
            SubagentPublication::Committed => None,
        };
        if let Some(failure) = settlement_failure {
            return Err(WorkflowRunError::ChildSettlement {
                node: node_id.to_string(),
                failure,
            });
        }
        match snapshot.state {
            crate::runtime::subagent::SubagentState::Succeeded => {
                // The committed output value is the live Workflow result
                // channel owned by the registry (Issue #178): the
                // observation snapshot deliberately never carries it.
                let value = self
                    .subagents
                    .take_workflow_agent_output(subagent_id, node_id)
                    .ok_or_else(|| WorkflowRunError::ChildFailed {
                        node: node_id.to_string(),
                        detail: "workflow Agent completed without committed output".to_owned(),
                    })?;
                // This validation is not redundant with
                // `validate_workflow_candidate` in the registry: the registry
                // revalidates the untrusted cross-process child wire frame
                // before its durable commit, while this validates the run's
                // result against the node's own frozen output authority at
                // consumption time. The two checks guard different trust
                // boundaries; both stay.
                if let Some(schema) = agent.terminal_schema() {
                    let validator = jsonschema::Validator::new(&schema).map_err(|error| {
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
                }
                let value = agent
                    .value(value)
                    .ok_or_else(|| WorkflowRunError::ChildFailed {
                        node: node_id.to_string(),
                        detail: "workflow Agent output does not match its output mode".to_owned(),
                    })?;
                // The native SubagentRegistry already committed the value
                // fact atomically with the child's terminal lifecycle fact.
                // Do not append a second observability event here: the
                // WorkflowRun consumes that durable handoff but is not a
                // second Event Journal authority.
                Ok(value)
            }
            crate::runtime::subagent::SubagentState::Cancelled => {
                // The registry's first committed cause is immutable. Only
                // inherited parent cancellation may project the parent's
                // richer deadline cause; an unrelated later outer cancellation
                // cannot replace a child's own committed cause.
                let status = match snapshot.cancel_reason {
                    Some(crate::runtime::types::CancellationReason::SubagentExecutionDeadlineExceeded) => Status::TimedOut,
                    Some(crate::runtime::types::CancellationReason::ParentCancelled)
                        if cancellation.is_cancelled() => cancellation.native_status(ToolCancellationPhase::DuringExecution),
                    Some(reason) => Status::Cancelled { reason, phase: ToolCancellationPhase::DuringExecution },
                    None => Status::OutcomeUnknown { detail: "native child cancellation has no committed cause".into() },
                };
                Err(WorkflowRunError::ChildOutcome {
                    node: node_id.to_string(),
                    status,
                })
            }
            crate::runtime::subagent::SubagentState::Interrupted => {
                Err(WorkflowRunError::ChildOutcome {
                    node: node_id.to_string(),
                    status: Status::OutcomeUnknown {
                        detail: bound_workflow_diagnostic(snapshot.detail.unwrap_or_else(|| {
                            "native child interrupted without a valid outcome".into()
                        })),
                    },
                })
            }
            state => Err(WorkflowRunError::ChildFailed {
                node: node_id.to_string(),
                detail: bound_workflow_diagnostic(
                    snapshot
                        .detail
                        .unwrap_or_else(|| format!("native child settled as {state:?}").into()),
                ),
            }),
        }
    }
}

/// Which independent native child settlement obligation is unproven.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowChildSettlementFailure {
    LogicalTerminalMissing,
    PublicationPending,
    PublicationAbandoned,
    PhysicalContainmentUnproven,
}

impl fmt::Display for WorkflowChildSettlementFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::LogicalTerminalMissing => "native child logical terminal did not commit",
            Self::PublicationPending => "native child terminal publication has not committed",
            Self::PublicationAbandoned => {
                "native child terminal publication was abandoned after durability failure"
            }
            Self::PhysicalContainmentUnproven => {
                "native child terminal publication committed but physical containment is unproven"
            }
        })
    }
}

/// A workflow execution error. Execution failures remain failures; they are
/// never converted into workflow-local values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowRunError {
    Disabled {
        workflow: WorkflowId,
        diagnostics: Vec<WorkflowAdmissionDiagnostic>,
    },
    LimitExceeded(WorkflowLimit),
    ChildOutcome {
        node: String,
        status: crate::tools::types::ToolExecutionStatus,
    },
    ChildSettlement {
        node: String,
        failure: WorkflowChildSettlementFailure,
    },
    CapabilityNotAdmitted(String),
    IneligibleCapability(String),
    IdentityChanged(String),
    InvocationAuthority(String),
    /// A native leaf settled without successful business output.
    ToolFailed {
        node: String,
        status: crate::tools::types::ToolExecutionStatus,
    },
    /// An Auxiliary Model Call failed without committing a local value.
    ModelFailed {
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
    /// Multiple admitted nodes failed, in node ID order.
    NodesFailed {
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
            Self::ToolFailed { status, .. }
            | Self::ModelFailed { status, .. }
            | Self::ChildOutcome { status, .. } => status.clone(),
            Self::ChildSettlement { failure, .. } => Status::OutcomeUnknown {
                detail: failure.to_string(),
            },
            Self::Deadline(_) => Status::TimedOut,
            Self::Cancelled(reason) => Status::Cancelled {
                reason: *reason,
                phase: ToolCancellationPhase::DuringExecution,
            },
            Self::NodesFailed { failures, .. } => {
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
            Self::Disabled {
                workflow,
                diagnostics,
            } => write!(
                formatter,
                "Workflow {workflow} is disabled: {}",
                serde_json::to_string(diagnostics).expect("typed diagnostics serialize")
            ),
            Self::LimitExceeded(limit) => {
                write!(formatter, "Workflow run {limit:?} limit exceeded")
            }
            Self::ChildOutcome { node, status } => {
                write!(formatter, "Workflow Agent {node:?}: {status:?}")
            }
            Self::ChildSettlement { node, failure } => {
                write!(formatter, "Workflow Agent {node:?}: {failure}")
            }
            Self::ToolFailed { node, status } => {
                write!(formatter, "Workflow Tool {node:?}: {status:?}")
            }
            Self::ModelFailed { node, status } => {
                write!(formatter, "Workflow Model {node:?}: {status:?}")
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
            Self::NodesFailed { failures } => {
                write!(formatter, "Workflow nodes failed: ")?;
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

fn bound_workflow_diagnostic(value: impl AsRef<str>) -> String {
    let value = value.as_ref();
    let mut end = value.len().min(MAX_WORKFLOW_DIAGNOSTIC_BYTES);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use crate::durable::{CanonicalStore, JournalStore};
    mod agents;
    mod authored_paths;
    mod compiler_models;
    mod graph;
    #[cfg(unix)]
    #[path = "model.rs"]
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
            timeout_ms: 600_000,
            description: "Test workflow".to_owned(),
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

    fn compile_test(
        definition: WorkflowDefinition,
    ) -> Result<WorkflowProgram, WorkflowCompileError> {
        WorkflowProgram::compile(WorkflowId::parse("test_workflow").expect("id"), definition)
    }

    #[cfg(unix)]
    const WORKFLOW_TEST_MODELS: &str = r#"[providers.local]
base_url = "http://127.0.0.1:9/v1"
api_key = "test-only-secret"

[models."local/model"]
provider = "local"
id = "model"
protocol = "openai_chat_completions"
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
        let mailbox =
            crate::runtime::inbound::ConversationInboundMailbox::over_store(store.clone());
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
                product_root: crate::runtime::local_storage::ProductRoot::create(
                    &runtime_root.clone(),
                )
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
        )
    }

    #[cfg(unix)]
    fn workflow_test_context_for_generation(
        plane: &WorkflowTestPlane,
        revision: u64,
        instructions: &str,
        workflow_catalog: WorkflowCatalog,
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
                project_instructions:
                    crate::runtime::agent_profile::AgentProjectInstructionPolicy {
                        inherit: false,
                        files: Vec::new(),
                    },
                extensions: crate::extensions::NativeAgentExtensionsDocument::default().resolve(),
                agents: std::collections::BTreeSet::default(),
                workflows: std::collections::BTreeSet::default(),
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
            .with_workflow_catalog(workflow_catalog),
        );
        crate::runtime::subagent::NativeExecutionContext::test_context(
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
        let output = schema(json!({"summary":{"type":"string"}}), &["summary"]);
        let mut nodes =
            BTreeMap::from([("workflow_start".into(), WorkflowNodeDefinition::Start {})]);
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
                .filter(|cut| matches!(
                    cut.runs[0].state,
                    read_model::WorkflowState::Settled { .. }
                ))
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
            WorkflowCatalog::new([(*old_program).clone()]).expect("old workflow catalog");
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

        let old_policy = old_context.execution_policy();
        let mut future = crate::local_runtime::config::CurrentRuntimeConfig::from_toml_slice(
            b"[agent.model]\nmodel = \"local/model\"\n",
        )
        .unwrap();
        future.model_timeout_policy.response_start_timeout_ms = 41_000;
        future.tool_deadline_policy.hard_deadline_ms = 42_000;
        future.context.reserve_tokens = 123;
        // Capture precedes publication; the Workflow's child is not prepared
        // until after this native registry publication has completed.
        plane.registry.publish_configuration(&future);
        let new_policy = crate::runtime::subagent::InheritedExecutionPolicy {
            model_timeout: future.timeout_policy().unwrap(),
            tool_deadline: future.tool_deadline_policy().unwrap(),
            context: future.context_policy(),
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
        let new_catalog =
            WorkflowCatalog::new([(*new_program).clone()]).expect("new workflow catalog");
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
        let source = include_str!("../../tests/fixtures/workflow/visual-authoring.json");
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
            assert!(plane.registry.all_snapshots().is_empty());
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
            "../../tests/fixtures/workflow/schema-rename.json"
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
            "../../tests/fixtures/workflow/schema-rename-collisions.json"
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
        apply_schema_defaults(&schema, &mut value);
        assert_eq!(
            value,
            json!({"tone": "formal", "rows": [{"n": 1}, {"n": 2}], "kept": "given"})
        );
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
        let program =
            WorkflowProgram::compile(WorkflowId::parse("review_pr").expect("id"), definition)
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
        let definition: WorkflowDefinition = serde_yaml::from_str(yaml).expect("yaml");
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
        assert!(serde_yaml::from_str::<WorkflowDefinition>(yaml).is_err());
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
                nodes: {
                    let mut nodes: std::collections::BTreeMap<
                        String,
                        crate::runtime::workflow::WorkflowNodeDefinition,
                    > = BTreeMap::from([
                        (
                            "review".to_owned(),
                            WorkflowNodeDefinition::Agent {
                                profile: "reviewer".into(),
                                task: vec![
                                    text("Review the input."),
                                    reference("args.task.detail"),
                                ],
                                output: WorkflowAgentOutput::JsonSchema {
                                    schema: schema(json!({}), &[]),
                                },
                                routes: Vec::new(),
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
}
