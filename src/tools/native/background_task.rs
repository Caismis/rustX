//! The `background_task` runtime intrinsic.
//!
//! Exactly one runtime-owned intrinsic exists: `background_task` with
//! canonical input
//!
//! ```json
//! {
//!   "execution_id": "exec_...",
//!   "action": "status | cancel"
//! }
//! ```
//!
//! There is no list and no delete operation. Its policies are fixed to
//! foreground-only sequential execution and it may never become
//! background-dispatchable (enforced by the registry). `status` returns the
//! canonical structured snapshot; `cancel` requests cancellation and returns
//! the canonical snapshot after processing the request without waiting for
//! final settlement. Cancellation is idempotent and never destructive;
//! unknown execution ids (including another conversation's ids, which are
//! indistinguishable from unknown ids) return a normal failed tool result.

use futures_util::future::BoxFuture;

use crate::runtime::identity::ToolExecutionId;
use crate::tools::background::ConversationBackgroundRegistry;
use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::types::{
    ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolExecutionResult,
    ToolExecutionStatus, ToolInvocation, ToolOrigin, ToolReplayPolicy, ToolResultContent,
};

/// The canonical model-facing name of the intrinsic.
pub const BACKGROUND_TASK_NAME: &str = crate::tools::executor::BACKGROUND_TASK_TOOL_NAME;

/// The canonical schema of the `background_task` intrinsic.
#[must_use]
pub fn definition() -> ToolDefinition {
    ToolDefinition {
        id: crate::runtime::identity::ToolId::new("tool-background-task"),
        name: BACKGROUND_TASK_NAME.to_owned(),
        description:
            "Inspect or cancel a background execution of this conversation by its execution id."
                .to_owned(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "execution_id": {"type": "string"},
                "action": {"type": "string", "enum": ["status", "cancel"]}
            },
            "required": ["execution_id", "action"],
            "additionalProperties": false
        }),
        execution_policy: ToolExecutionPolicy::ForegroundOnly,
        concurrency_policy: ToolConcurrencyPolicy::Sequential,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Builtin,
    }
}

/// The executor of the `background_task` intrinsic.
///
/// The executor holds a handle to the conversation-owned background
/// registry; lookup is scoped to that conversation by construction, so
/// another conversation's execution id is indistinguishable from an unknown
/// id.
pub struct BackgroundTaskExecutor {
    background: ConversationBackgroundRegistry,
}

impl BackgroundTaskExecutor {
    /// Creates the intrinsic executor over the conversation background
    /// registry.
    #[must_use]
    pub fn new(background: ConversationBackgroundRegistry) -> Self {
        Self { background }
    }
}

impl ToolExecutor for BackgroundTaskExecutor {
    fn execute<'a>(
        &'a self,
        invocation: ToolInvocation,
        _context: ToolExecutionContext<'a>,
    ) -> BoxFuture<'a, ToolExecutionResult> {
        let background = self.background.clone();
        Box::pin(async move { run_background_task(&background, &invocation.arguments) })
    }
}

/// Runs one `background_task` invocation against the conversation registry.
fn run_background_task(
    background: &ConversationBackgroundRegistry,
    arguments: &serde_json::Value,
) -> ToolExecutionResult {
    let Some(object) = arguments.as_object() else {
        return failed("background_task arguments must be an object");
    };
    let Some(execution_id) = object
        .get("execution_id")
        .and_then(serde_json::Value::as_str)
    else {
        return failed("background_task requires a string execution_id");
    };
    let Some(action) = object.get("action").and_then(serde_json::Value::as_str) else {
        return failed("background_task requires an action");
    };
    let execution_id = ToolExecutionId::new(execution_id);
    match action {
        "status" => match background.snapshot(&execution_id) {
            Some(snapshot) => snapshot_result(snapshot),
            None => failed(format!(
                "unknown background execution {}",
                execution_id.as_str()
            )),
        },
        "cancel" => match background.cancel(&execution_id) {
            Some(snapshot) => snapshot_result(snapshot),
            None => failed(format!(
                "unknown background execution {}",
                execution_id.as_str()
            )),
        },
        other => failed(format!("unknown background_task action {other:?}")),
    }
}

/// The canonical snapshot result: the full structured snapshot.
fn snapshot_result(
    snapshot: crate::tools::background::BackgroundExecutionSnapshot,
) -> ToolExecutionResult {
    ToolExecutionResult {
        status: ToolExecutionStatus::Success,
        content: vec![ToolResultContent::Json {
            value: serde_json::to_value(snapshot).expect("background snapshots serialize"),
        }],
        duration_ms: 0,
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
    }
}

fn failed(error: impl Into<String>) -> ToolExecutionResult {
    ToolExecutionResult {
        status: ToolExecutionStatus::Failed {
            error: error.into(),
        },
        content: Vec::new(),
        duration_ms: 0,
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
    }
}
