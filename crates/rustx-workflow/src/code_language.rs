//! The Workflow code-node language contract.

/// Typed Python/JavaScript main functions over the admitted workspace Bash tool.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowCodeLanguage {
    Python3,
    JavaScript,
}
