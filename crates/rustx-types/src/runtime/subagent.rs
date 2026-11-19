//! Subagent admission vocabulary shared across layers.

use crate::runtime::identity::ToolCallId;

/// Truthful admission source. Client controls have no model `ToolCall` identity.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentActivationOrigin {
    CreationTool {
        tool_call_id: ToolCallId,
    },
    MessageTool {
        tool_call_id: ToolCallId,
    },
    ClientControl,
    Workflow {
        node_id: Box<crate::runtime::workflow::WorkflowNodeInstance>,
    },
}

impl AgentActivationOrigin {
    #[must_use]
    pub fn tool_call_id(&self) -> Option<&ToolCallId> {
        match self {
            Self::CreationTool { tool_call_id } | Self::MessageTool { tool_call_id } => {
                Some(tool_call_id)
            }
            Self::ClientControl | Self::Workflow { .. } => None,
        }
    }
}
