//! Bounded list/aggregate node vocabulary of the Workflow transform grammar.

use serde::{Deserialize, Serialize};

use crate::definition::{WorkflowValue, WorkflowVarType};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowAggregateGroup {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_type: Option<WorkflowVarType>,
    pub variables: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowListOperator {
    Equal,
    NotEqual,
    Contains,
    NotContains,
    StartsWith,
    EndsWith,
    Greater,
    GreaterEqual,
    Less,
    LessEqual,
    Empty,
    NotEmpty,
    In,
    NotIn,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowListFilter {
    /// Empty selects a primitive item; otherwise literal object field names.
    pub path: Vec<String>,
    pub operator: WorkflowListOperator,
    pub value: WorkflowValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowSortOrder {
    Asc,
    Desc,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowListSort {
    pub path: Vec<String>,
    pub order: WorkflowSortOrder,
}
