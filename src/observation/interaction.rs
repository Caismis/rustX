//! Finite identity headers of live native interactions.
use super::bounded::bounded_clone;
use crate::runtime::identity::{ConversationId, ToolId};
use crate::runtime::interaction::{InteractionRef, InteractionRequest, InteractionSource};
use serde::{Deserialize, Serialize};

/// Constructs only finite identity metadata under the Projection boundary.
/// Full immutable native bodies are source-owned and read independently.
#[cfg(test)]
pub(crate) fn interaction_header(
    value: &crate::runtime::interaction::RoutedInteraction,
) -> super::interaction::RuntimeClientInteractionHeader {
    interaction_request_header(&value.request, value.source.clone())
}
pub(crate) fn interaction_request_header(
    request: &crate::runtime::interaction::InteractionRequest,
    source: crate::runtime::interaction::InteractionSource,
) -> super::interaction::RuntimeClientInteractionHeader {
    use super::interaction::{
        RuntimeClientInteractionHeader as Header, RuntimeClientInteractionKind as Kind,
    };
    use crate::runtime::interaction::InteractionKind;
    let (kind, original_node, node_path) = match &request.kind {
        InteractionKind::Approval { invocation_id, .. } => (
            Kind::Approval,
            match invocation_id {
                crate::tools::types::ToolInvocationId::Workflow { node } => Some(node.as_ref()),
                crate::tools::types::ToolInvocationId::Agent { .. } => None,
            },
            vec![
                "request".into(),
                "kind".into(),
                "invocation_id".into(),
                "node".into(),
            ],
        ),
        InteractionKind::Questionnaire { invocation_id, .. } => (
            Kind::Questionnaire,
            match invocation_id {
                crate::tools::types::ToolInvocationId::Workflow { node } => Some(node.as_ref()),
                crate::tools::types::ToolInvocationId::Agent { .. } => None,
            },
            vec![
                "request".into(),
                "kind".into(),
                "invocation_id".into(),
                "node".into(),
            ],
        ),
        InteractionKind::Review { review, .. } => (
            Kind::Review,
            Some(review.instance.as_ref()),
            vec![
                "request".into(),
                "kind".into(),
                "review".into(),
                "instance".into(),
            ],
        ),
    };
    let workflow_node = original_node
        .and_then(|node| bounded_clone(node, 1024))
        .map(Box::new);
    let workflow_node_deferred = original_node.is_some() && workflow_node.is_none();
    Header {
        source_offset: 0,
        interaction: request.interaction_ref(),
        source,
        request_id: request.id.clone(),
        conversation_id: request.conversation_id.clone(),
        execution: request.execution.clone(),
        kind,
        approval: match &request.kind {
            InteractionKind::Approval {
                tool_id,
                tool_name,
                reason,
                mode,
                ..
            } => Some(super::interaction::RuntimeClientApprovalSummary {
                tool_id: super::tool::inline_tool_id(tool_id),
                tool_name: tool_name.chars().take(256).collect(),
                reason: reason.chars().take(256).collect(),
                mode: *mode,
                deferred_fields: {
                    let mut fields =
                        vec![vec!["request".into(), "kind".into(), "arguments".into()]];
                    if tool_id.as_str().len() > 256 {
                        fields.push(vec!["request".into(), "kind".into(), "tool_id".into()]);
                    }
                    if tool_name.len() > 256 {
                        fields.push(vec!["request".into(), "kind".into(), "tool_name".into()]);
                    }
                    if reason.len() > 256 {
                        fields.push(vec!["request".into(), "kind".into(), "reason".into()]);
                    }
                    fields
                },
            }),
            _ => None,
        },
        body: None,
        body_complete: false,
        workflow_node,
        workflow_node_deferred,
        workflow_node_path: workflow_node_deferred.then_some(node_path),
    }
}

/// Finite display identity of an actionable native request. A missing body is
/// an explicit deferred source, never an empty or reconstructed Questionnaire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientInteractionHeader {
    /// Exact native source offset, stable for this pending lifetime.
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub source_offset: u64,
    pub interaction: InteractionRef,
    pub source: InteractionSource,
    pub request_id: crate::runtime::identity::InteractionId,
    pub conversation_id: ConversationId,
    pub execution: crate::runtime::interaction::InteractionExecution,
    pub kind: RuntimeClientInteractionKind,
    pub approval: Option<RuntimeClientApprovalSummary>,
    pub body: Option<InteractionRequest>,
    pub body_complete: bool,
    pub workflow_node: Option<Box<crate::runtime::workflow::WorkflowNodeInstance>>,
    pub workflow_node_deferred: bool,
    pub workflow_node_path: Option<Vec<String>>,
}
/// Finite decision metadata; business arguments remain an exact deferred native source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientApprovalSummary {
    pub tool_id: Option<ToolId>,
    pub tool_name: String,
    pub reason: String,
    pub mode: crate::tools::types::ToolInvocationMode,
    pub deferred_fields: Vec<Vec<String>>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeClientInteractionKind {
    Approval,
    Questionnaire,
    Review,
}
