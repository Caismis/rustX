//! Thin Agent-domain controls. The registry owns activation arbitration.
use super::input::decode;
use super::registration::{NativeToolRegistration, input_schema};
use super::support::{cancelled_result, failed_result, success_json};
use crate::runtime::identity::{AgentId, ToolId};
use crate::runtime::subagent::SubagentRegistry;
use crate::tools::deadline::ToolProgressCapability;
use crate::tools::executor::{ToolExecutionContext, ToolExecutionHandle, ToolExecutor};
use crate::tools::types::{
    ToolApprovalPolicy, ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolInvocation,
    ToolOrigin, ToolReplayPolicy,
};
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;

pub(crate) const NAMES: [&str; 4] = [
    "list_agents",
    "send_message",
    "wait_agent",
    "interrupt_agent",
];
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListInput {}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TargetInput {
    agent_id: AgentId,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MessageInput {
    agent_id: AgentId,
    message: String,
}

pub(super) fn definitions() -> Vec<ToolDefinition> {
    [
        (NAMES[0], "List this conversation's durable child Agents, bounded to 64 stable identities. Newest-created first, with matched/truncated counts. Active accepts messages; Admitting and Stopping reject new messages transiently; Inactive resumes through send_message.", input_schema::<ListInput>()),
        (NAMES[1], "Send input to a durable child Agent. The owner atomically admits it to the current activation or starts one activation of the same inactive child conversation. Admitting and Stopping reject new messages transiently. Success means the child durably accepted the input; do not choose steer versus resume.", input_schema::<MessageInput>()),
        (NAMES[2], "Wait for the reserved or current activation captured by this operation to physically settle. A later resumed activation cannot extend this wait. Inactive returns immediately.", input_schema::<TargetInput>()),
        (NAMES[3], "Interrupt the exact reserved admission or current activation captured by this operation and wait for physical settlement. The durable Agent remains available for later send_message.", input_schema::<TargetInput>()),
    ].into_iter().map(|(name, description, input_schema)| ToolDefinition {
        id: ToolId::new(format!("tool-{name}")), name: name.into(), description: description.into(), input_schema,
        execution_policy: ToolExecutionPolicy::ForegroundOnly, concurrency_policy: ToolConcurrencyPolicy::Sequential,
        approval_policy: ToolApprovalPolicy::Never, replay_policy: ToolReplayPolicy::Never, origin: ToolOrigin::Builtin,
    }).collect()
}

pub(super) fn registrations(registry: &SubagentRegistry) -> Vec<NativeToolRegistration> {
    definitions()
        .into_iter()
        .map(|definition| {
            NativeToolRegistration::new(definition, Arc::new(AgentExecutor(registry.clone())))
        })
        .collect()
}
struct AgentExecutor(SubagentRegistry);
impl ToolExecutor for AgentExecutor {
    fn progress_capability(&self) -> ToolProgressCapability {
        ToolProgressCapability::None
    }
    fn start<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionHandle<'a> {
        let cancellation = context.cancellation.clone();
        ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                if invocation.tool_name == NAMES[0] {
                    if let Err(error) = decode::<ListInput>(NAMES[0], &invocation.arguments) {
                        return failed_result(error);
                    }
                    let listing = self.0.list_agents(64);
                    return success_json(serde_json::json!({
                        "returned": listing.agents.len(), "matched": listing.matched,
                        "truncated": listing.matched > listing.agents.len(), "limit":64,
                        "agents": listing.agents,
                    }));
                }
                if invocation.tool_name == NAMES[1] {
                    let input = match decode::<MessageInput>(NAMES[1], &invocation.arguments) {
                        Ok(input) => input,
                        Err(error) => return failed_result(error),
                    };
                    let input_cancellation = cancellation.child_signal();
                    let origin = crate::runtime::subagent::AgentActivationOrigin::MessageTool {
                        tool_call_id: invocation
                            .id
                            .canonical_call_id()
                            .expect("Agent-owned invocation")
                            .clone(),
                    };
                    return tokio::select! {
                        biased;
                        result = self.0.send_message(&input.agent_id, &input.message, origin, input_cancellation) => match result {
                            Ok(accepted) => success_json(serde_json::json!(accepted)),
                            Err(error) => failed_result(error.to_string()),
                        },
                        () = cancellation.cancelled() => cancelled_result(cancellation.reason()),
                    };
                }
                let input =
                    match decode::<TargetInput>(&invocation.tool_name, &invocation.arguments) {
                        Ok(input) => input,
                        Err(error) => return failed_result(error),
                    };
                let operation = async {
                    if invocation.tool_name == NAMES[3] {
                        self.0.interrupt_agent(&input.agent_id).await
                    } else {
                        self.0.wait_agent(&input.agent_id).await
                    }
                };
                tokio::select! {
                    result = operation => match result {
                        Ok(result) => success_json(serde_json::json!({
                            "agent_id": result.agent_id, "activation_id": result.activation_id,
                            "outcome": result.outcome.map(|snapshot| snapshot.state),
                        })),
                        Err(error) => failed_result(error.to_string()),
                    },
                    () = cancellation.cancelled() => cancelled_result(cancellation.reason()),
                }
            }),
            context.cancellation.clone(),
        )
    }
}
