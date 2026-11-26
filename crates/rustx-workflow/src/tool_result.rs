//! The Tool-node result declaration contract.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What a Tool node commits, fixed by the Tool: its declared structured
/// result, or its text. Never provider text heuristics or a part index.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub enum WorkflowToolResult {
    /// The result's one native structured JSON part, satisfying this schema.
    Json { schema: Value },
    /// `{text}`: the result's native text parts, joined by newlines.
    Text {},
}
