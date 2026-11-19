//! The Workflow authoring contract: the serialized graph and node vocabulary a
//! `WorkflowFileDocument` freezes. Execution, compilation and runtime evidence
//! live outside this crate; everything here is pure declarative data plus the
//! bounded construction rules the file format itself enforces.

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::code_language::WorkflowCodeLanguage;
use crate::list::{WorkflowAggregateGroup, WorkflowListFilter, WorkflowListSort};
use crate::review::WorkflowReviewAction;
use crate::selections::{WorkflowModelPolicy, WorkflowToolSelector};
use crate::tool_result::WorkflowToolResult;
use rustx_types::runtime::permission::resource::ResourcePermissionSet;
use rustx_types::runtime::workflow::WorkflowPort;

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
    /// Source-default three-mode permission object for this workflow Tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<ResourcePermissionSet>,
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
    #[serde(default = "empty_object_schema")]
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
    /// Jinja-style text transformation with explicit lexical inputs.
    Template {
        variables: BTreeMap<String, WorkflowValue>,
        template: String,
    },
    /// Pure filter, stable sort and selection over an array.
    List {
        input: WorkflowValue,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<WorkflowListFilter>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sort: Option<WorkflowListSort>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        extract: Option<WorkflowValue>,
    },
    /// One tool-free classification Call with an exit per class.
    Classifier {
        #[serde(default)]
        model: WorkflowModelPolicy,
        input: WorkflowValue,
        #[serde(default)]
        instruction: Vec<WorkflowValue>,
        classes: Vec<WorkflowClassifierClass>,
    },
    /// A Python/JavaScript main function executed through workspace Bash.
    Code {
        code_language: WorkflowCodeLanguage,
        variables: BTreeMap<String, WorkflowValue>,
        code: String,
        outputs: BTreeMap<String, WorkflowVarType>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry: Option<WorkflowRetry>,
        #[serde(default)]
        on_error: WorkflowValueErrorStrategy,
    },
    /// Select the first available source per output, optionally grouped.
    Aggregate {
        grouped: bool,
        groups: Vec<WorkflowAggregateGroup>,
    },
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
        /// Effective resource permission for this Agent entry only.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        permissions: Option<ResourcePermissionSet>,
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

pub fn empty_object_schema() -> Value {
    serde_json::json!({"type": "object", "properties": {}, "additionalProperties": false})
}

/// Exactly what an omitted schema reads back as.
/// A closed object with no fields, however its empty `required` is spelled.
pub fn is_empty_object_schema(schema: &Value) -> bool {
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
pub fn loop_variables_schema(variables: &[WorkflowLoopVariable]) -> Value {
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

/// A classifier's stable exit, display label and variable-aware topic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowClassifierClass {
    pub id: String,
    pub label: String,
    pub description: Vec<WorkflowValue>,
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
    Greater {
        left: WorkflowValue,
        right: WorkflowValue,
    },
    GreaterEqual {
        left: WorkflowValue,
        right: WorkflowValue,
    },
    Less {
        left: WorkflowValue,
        right: WorkflowValue,
    },
    LessEqual {
        left: WorkflowValue,
        right: WorkflowValue,
    },
    /// String or typed-array containment, without coercion.
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
    /// Explicit emptiness of a JSON value, as Dify tests it.
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

/// The default total foreground lifetime of one workflow run (10 minutes).
pub const fn default_workflow_timeout_ms() -> u64 {
    600_000
}
