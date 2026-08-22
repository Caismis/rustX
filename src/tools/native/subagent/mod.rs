//! The `subagent` runtime intrinsic (Issue #60).
//!
//! The model-facing surface of the native async one-shot subagent plane:
//!
//! ```json
//! {
//!   "profile": "explore",
//!   "task": "...",
//!   "context": "..."   // optional, bounded
//! }
//! ```
//!
//! The call returns **immediately after the ownership commit** with a
//! running handle — the child runtime works asynchronously and its bounded
//! final answer arrives later as an ordinary inbound turn from the child
//! agent. There is no wait/poll mode and no result channel outside the
//! conversation's own message bus.
//!
//! The executor is a thin adapter over the conversation-owned
//! [`SubagentRegistry`]: input validation, profile resolution, the
//! two-stage prepare/commit boundary, and the cancellation-race outcome
//! mapping. All lifecycle, durability, and supervision semantics live in
//! the registry.

use futures_util::future::BoxFuture;

use crate::runtime::subagent::{
    SubagentProfile, SubagentRegistry, SubagentStartError, SubagentStartOutcome, SubagentStartSpec,
};
use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::native::registration::{NativeToolRegistration, input_schema};
use crate::tools::types::{
    ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolExecutionResult,
    ToolExecutionStatus, ToolInvocation, ToolOrigin, ToolReplayPolicy,
};

use super::input::decode;
use super::support::{failed_result, success_json};

/// The canonical model-facing name of the intrinsic.
pub const SUBAGENT_TOOL_NAME: &str = "subagent";

/// The tool-owned registration of the `subagent` runtime intrinsic.
///
/// The intrinsic owns its own fixed policies (foreground-only, sequential):
/// the async boundary is the child runtime, not the tool execution.
#[must_use]
pub(super) fn registration(subagents: SubagentRegistry) -> NativeToolRegistration {
    NativeToolRegistration::new(
        definition(),
        std::sync::Arc::new(SubagentExecutor { subagents }),
    )
}

/// The canonical schema of the `subagent` intrinsic.
fn definition() -> ToolDefinition {
    ToolDefinition {
        id: crate::runtime::identity::ToolId::new("tool-subagent"),
        name: SUBAGENT_TOOL_NAME.to_owned(),
        description: concat!(
            "Delegate a bounded read-only task to a one-shot child agent runtime. ",
            "The child runs asynchronously in its own isolated conversation and process; ",
            "this call returns as soon as the child is durably started, and the child's ",
            "final answer arrives later as a new message from the child agent. The v1 ",
            "'explore' profile can inspect the workspace with Read/Glob/Grep only."
        )
        .to_owned(),
        input_schema: input_schema::<SubagentInput>(),
        execution_policy: ToolExecutionPolicy::ForegroundOnly,
        concurrency_policy: ToolConcurrencyPolicy::Sequential,
        approval_policy: crate::tools::types::ToolApprovalPolicy::Never,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Builtin,
    }
}

/// The typed model-facing input contract of the `subagent` intrinsic.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct SubagentInput {
    /// The execution profile of the child. v1 supports exactly `explore`.
    pub profile: String,
    /// The delegated task, in natural language.
    pub task: String,
    /// An explicit bounded context package for the child.
    #[serde(default)]
    pub context: Option<String>,
}

impl SubagentInput {
    /// Deserializes one `subagent` invocation.
    ///
    /// # Errors
    ///
    /// Returns the deterministic rejection message of the first input
    /// contract violation.
    fn parse(arguments: &serde_json::Value) -> Result<Self, String> {
        decode(SUBAGENT_TOOL_NAME, arguments)
    }
}

/// The executor of the `subagent` intrinsic.
struct SubagentExecutor {
    subagents: SubagentRegistry,
}

impl ToolExecutor for SubagentExecutor {
    fn execute<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> BoxFuture<'a, ToolExecutionResult> {
        Box::pin(async move {
            let input = match SubagentInput::parse(&invocation.arguments) {
                Ok(input) => input,
                Err(error) => return failed_result(error),
            };
            let Some(profile) = SubagentProfile::by_name(&input.profile) else {
                return failed_result(format!(
                    "unknown subagent profile {:?}: v1 supports exactly \"explore\"",
                    input.profile
                ));
            };
            let spec = SubagentStartSpec {
                profile,
                task: input.task,
                context: input.context,
                tool_call_id: invocation.call_id.clone(),
            };
            let prepared = match self.subagents.prepare(&spec).await {
                Ok(prepared) => prepared,
                Err(error) => return failed_result(error.to_string()),
            };
            match self
                .subagents
                .commit(prepared, &context.cancellation.signal())
                .await
            {
                Ok(SubagentStartOutcome::Accepted(accepted)) => {
                    let mut result = accepted.result;
                    result["subagent_id"] =
                        serde_json::Value::String(accepted.subagent_id.to_string());
                    result["child_agent_id"] =
                        serde_json::Value::String(accepted.child_agent_id.to_string());
                    result["profile"] = serde_json::Value::String(accepted.profile);
                    success_json(result)
                }
                // The attempt cancellation won the race against the
                // ownership commit: nothing was published, the staged child
                // is already torn down, and the tool result is the
                // absorbing cancellation outcome.
                Ok(SubagentStartOutcome::RolledBack) => ToolExecutionResult {
                    status: ToolExecutionStatus::Cancelled {
                        reason: context.cancellation.reason(),
                    },
                    content: Vec::new(),
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                    managed_output: None,
                },
                Err(SubagentStartError::ConversationInactive) => {
                    failed_result("the conversation is shutting down")
                }
                Err(error) => failed_result(error.to_string()),
            }
        })
    }
}
