//! Workflow-owned identity, run-instance and limit contracts shared with
//! tool and scheduler types.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};
#[cfg(any(test, feature = "test-fixtures"))]
use serde_json::Value;

pub mod scheduled;

/// The reserved Tool name of the Workflow Agent terminal protocol.
pub const WORKFLOW_OUTPUT_TOOL_NAME: &str = "workflow_output";

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
        attempt_id: rustx_identity::AttemptId,
    },
    Scheduled {
        start_id: scheduled::lifecycle::ScheduledWorkflowStartId,
    },
}
impl WorkflowRunOrigin {
    #[must_use]
    pub fn attempt_id(&self) -> Option<&rustx_identity::AttemptId> {
        match self {
            Self::Tool { attempt_id } => Some(attempt_id),
            Self::Scheduled { .. } => None,
        }
    }
}
impl From<rustx_identity::AttemptId> for WorkflowRunOrigin {
    fn from(attempt_id: rustx_identity::AttemptId) -> Self {
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
    pub conversation_id: rustx_identity::ConversationId,
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

impl WorkflowBlockInstance {
    /// True when block nesting matches the compiler/executor contract: root plus up to
    /// `MAX_BLOCK_DEPTH` two-segment nested bodies (`invocations.len() == blocks.len() / 2 + 1`).
    pub fn has_valid_nesting(&self) -> bool {
        self.definition.blocks.len().is_multiple_of(2)
            && self.definition.blocks.len() <= 2 * MAX_BLOCK_DEPTH
            && self.invocations.len() == self.definition.blocks.len() / 2 + 1
    }
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

#[cfg(any(test, feature = "test-fixtures"))]
pub fn test_instance(workflow: &str, node: &str) -> WorkflowNodeInstance {
    WorkflowNodeInstance {
        block: WorkflowBlockInstance {
            run: WorkflowRunId {
                conversation_id: rustx_identity::ConversationId::new(
                    "conv_47f0e826-cb00-7b0b-8ce4-af52ffdacf51",
                ),
                origin: (rustx_identity::AttemptId::new("test-workflow-execution")).into(),
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
#[cfg(any(test, feature = "test-fixtures"))]
pub fn test_body(block: &Value) -> Value {
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
pub const MAX_WORKFLOW_BYTES: usize = 16 * 1024 * 1024;
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

/// The published Workflow read-model vocabulary carried in event payloads.
pub mod read_model {
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};

    /// What a node is currently waiting for.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    #[derive(JsonSchema)]
    pub enum WorkflowWait {
        Tool,
        Agent,
        Model,
        Capacity,
        Questionnaire,
        Approval,
        Review,
        Settlement,
    }

    /// The fixed Workflow node kinds.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    #[derive(JsonSchema)]
    pub enum WorkflowNodeKind {
        Template,
        List,
        Aggregate,
        Classifier,
        Block,
        Agent,
        Model,
        Tool,
        Branch,
        Start,
        Review,
        Loop,
        Iteration,
        LoopEnd,
        Assigner,
        Return,
    }
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

use serde::Serializer;

impl WorkflowPort {
    #[must_use]
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

/// Whether a string is a valid local Workflow key (port, label, node id).
#[must_use]
pub fn valid_local_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}
