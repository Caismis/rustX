//! Addressing inside a Workflow definition's nested block structure: pure
//! navigation, no compilation or execution facts.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::definition::{WorkflowBlock, WorkflowNodeDefinition};

/// Structured lexical address of a Loop or Iteration body.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct WorkflowBlockStep {
    pub node: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct WorkflowNodeAddress {
    pub block: Vec<WorkflowBlockStep>,
    pub node: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowConsumerField {
    TemplateVariable {
        name: String,
    },
    CodeVariable {
        name: String,
    },
    TransformInput,
    ListFilterValue,
    ListExtract,
    ClassifierInstruction,
    ClassifierClass {
        index: u32,
    },
    AggregateVariable {
        group: u32,
        index: u32,
    },
    /// A variable in one Model prompt message. Every part of a message shares
    /// its scope, so this also answers for a variable not inserted yet.
    Message {
        index: u32,
    },
    /// A variable in an Agent's task, answered as for a Model message.
    Task,
    Arguments,
    /// A variable in a Human Input's content, answered as for a Model message.
    ReviewContent,
    Condition {
        case_id: String,
    },
    /// Select a comparable variable independently of the current counterpart.
    ConditionVariable {
        case_id: String,
    },
    ReturnOutput,
    /// The value of one Loop variable.
    LoopVariable {
        index: u32,
    },
    /// The Loop's break condition, read in its body's scope.
    BreakCondition,
    /// Select a comparable variable of the break condition.
    BreakConditionVariable,
    IterationIterator,
    /// The Iteration's output, read in its body's scope.
    IterationOutput,
    /// The operand of one Variable Assigner write.
    AssignerValue {
        index: u32,
    },
}

/// Exact authored position within a value construction or predicate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowExpressionStep {
    Field { key: String },
    Item { index: u32 },
    Value,
    Left,
    Right,
    Predicate,
    PredicateAt { index: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowConsumerAddress {
    pub address: WorkflowNodeAddress,
    pub field: WorkflowConsumerField,
    #[serde(default)]
    pub path: Vec<WorkflowExpressionStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowAnalysisQuery {
    pub consumer: WorkflowConsumerAddress,
    #[serde(default)]
    pub search: String,
    /// An existing/manual reference to classify even if it is absent from the
    /// declaration catalog. This never creates fallback or indexing semantics.
    #[serde(default)]
    pub reference: Option<Vec<String>>,
    pub limit: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowReferenceAvailability {
    Available,
    Unavailable,
    Pending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowReferenceCompatibility {
    Compatible,
    Incompatible,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowOutputDeclaration {
    pub producer: WorkflowNodeAddress,
    /// None means an invalid/incomplete output contract, never an empty schema.
    #[schemars(with = "Option<BTreeMap<String, Value>>")]
    pub schema: Option<Value>,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowReferenceChoice {
    /// `args`/`result` have no executable producer address.
    pub producer: Option<WorkflowNodeAddress>,
    pub path: Vec<String>,
    #[schemars(with = "Option<BTreeMap<String, Value>>")]
    pub schema: Option<Value>,
    pub availability: WorkflowReferenceAvailability,
    pub compatibility: WorkflowReferenceCompatibility,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowCandidateAnalysis {
    pub outputs: Vec<WorkflowOutputDeclaration>,
    pub references: Vec<WorkflowReferenceChoice>,
    pub selected: Option<WorkflowReferenceChoice>,
    /// Lexical proof failure or invalid consumer; never an execution verdict.
    pub diagnostic: Option<String>,
    pub incomplete: bool,
    pub truncated: bool,
}

/// Resolve an address without interpreting local node ids as global identities.
#[must_use]
pub fn block_at<'a>(
    root: &'a WorkflowBlock,
    path: &[WorkflowBlockStep],
) -> Option<&'a WorkflowBlock> {
    let mut block = root;
    for step in path {
        block = match block.nodes.get(&step.node)? {
            WorkflowNodeDefinition::Loop { body, .. }
            | WorkflowNodeDefinition::Iteration { body, .. } => body,
            _ => return None,
        };
    }
    Some(block)
}

fn nav_valid_key(key: &str) -> bool {
    rustx_types::runtime::workflow::valid_local_key(key)
}
const MAX_ANALYSIS_CHOICES: usize = 256;
fn nav_valid_address(address: &WorkflowNodeAddress) -> bool {
    address.block.len() <= rustx_types::runtime::workflow::MAX_BLOCK_DEPTH
        && nav_valid_key(&address.node)
        && address.block.iter().all(|step| nav_valid_key(&step.node))
}
impl WorkflowAnalysisQuery {
    /// # Errors
    /// Rejects query depth/count/text beyond the finite authoring envelope.
    pub fn validate(&self) -> Result<(), &'static str> {
        if serde_json::to_vec(self).map_or(true, |bytes| bytes.len() > 16 * 1024) {
            return Err("analysis query exceeds 16 KiB");
        }
        if !nav_valid_address(&self.consumer.address) || self.consumer.path.len() > rustx_types::runtime::workflow::MAX_VALUE_DEPTH
            || self.search.len() > 256 || self.limit == 0 || self.limit > MAX_ANALYSIS_CHOICES
            || self.reference.as_ref().is_some_and(|path| path.is_empty() || path.len() > rustx_types::runtime::workflow::MAX_REFERENCE_COMPONENTS || path.iter().any(|s| s.is_empty() || s.len() > 64))
            || self.consumer.path.iter().any(|step| matches!(step, WorkflowExpressionStep::Field { key } if key.is_empty() || key.len() > 64)) {
            return Err("analysis query exceeds address, text, depth or result bounds");
        }
        match &self.consumer.field {
            WorkflowConsumerField::Condition { case_id }
            | WorkflowConsumerField::ConditionVariable { case_id }
                if !nav_valid_key(case_id) =>
            {
                Err("invalid case identity")
            }
            _ => Ok(()),
        }
    }
}
