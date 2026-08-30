//! The `execution` runtime intrinsic (Issue #162).
//!
//! `execution` is the single model-facing observation and cancellation
//! control plane for conversation-owned asynchronous executions. It is a
//! **control-plane router only**: it owns model-facing schema/input
//! validation, explicit target-kind dispatch, conversion into the owning
//! domain's id type, invocation of the owning registry API, and conversion
//! of the authoritative domain snapshot into a bounded tagged model-facing
//! response.
//!
//! It owns **no lifecycle state, no task/process handles, no cancellation
//! tokens, no cancellation implementation, no durability, no terminal
//! settlement, and no result publication**. Every request is routed to the
//! domain authority that owns the execution:
//!
//! ```text
//! model
//!   |
//!   v
//! execution intrinsic
//!   |
//!   +-----------------------------+
//!   |                             |
//!   | kind = tool                 | kind = subagent
//!   v                             v
//! ConversationBackgroundRegistry  SubagentRegistry
//!   |                             |
//!   | authoritative state/cancel  | authoritative state/cancel
//!   v                             v
//! BackgroundExecutionSnapshot     SubagentSnapshot
//! ```
//!
//! The canonical input is
//!
//! ```json
//! {
//!   "action": "status | cancel",
//!   "target": {
//!     "kind": "tool | subagent",
//!     "id": "..."
//!   }
//! }
//! ```
//!
//! The target kind is explicit and closed. The intrinsic never infers a
//! kind from an id prefix and never tries one registry and falls through to
//! another: a mismatched kind/id pair fails through the selected domain
//! authority exactly like an unknown id, and cross-conversation ids remain
//! indistinguishable from unknown ids at the owning domain boundary.
//!
//! `status` is observation; `cancel` is control. Neither is a result
//! channel: a subagent's final answer still arrives exactly once through
//! the existing canonical inbound message path.
//!
//! The intrinsic's policies are fixed to foreground-only sequential
//! execution and it may never become background-dispatchable (enforced by
//! the registry).

mod input;

use futures_util::future::BoxFuture;

use crate::runtime::identity::{SubagentId, ToolExecutionId};
use crate::runtime::subagent::SubagentRegistry;
use crate::runtime::types::CancellationReason;
use crate::tools::background::{BackgroundExecutionSnapshot, ConversationBackgroundRegistry};
use crate::tools::execution::ExecutionKind;
use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::native::registration::{NativeToolRegistration, input_schema};
use crate::tools::types::{
    ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolExecutionResult,
    ToolExecutionStatus, ToolInvocation, ToolOrigin, ToolReplayPolicy, ToolResultContent,
};

use input::{ExecutionAction, ExecutionInput};

/// The canonical model-facing name of the intrinsic.
pub const EXECUTION_TOOL_NAME: &str = crate::tools::executor::EXECUTION_TOOL_NAME;

/// The tool-owned registration of the `execution` runtime intrinsic.
///
/// The intrinsic owns its own fixed policies (foreground-only, sequential):
/// unlike the ordinary native tools it takes no configurable policy, and the
/// registry independently enforces the same fixed policies.
///
/// `subagents` is the conversation's subagent registry when this runtime
/// owns one (never inside a subagent child). Without one, subagent targets
/// fail deterministically as unknown — the runtime can never have owned a
/// subagent it cannot name.
#[must_use]
pub(crate) fn registration(
    background: ConversationBackgroundRegistry,
    subagents: Option<SubagentRegistry>,
) -> NativeToolRegistration {
    NativeToolRegistration::new(
        definition(),
        std::sync::Arc::new(ExecutionExecutor::new(background, subagents)),
    )
}

/// The canonical schema of the `execution` intrinsic.
fn definition() -> ToolDefinition {
    ToolDefinition {
        id: crate::runtime::identity::ToolId::new("tool-execution"),
        name: EXECUTION_TOOL_NAME.to_owned(),
        description:
            "Inspect or cancel a conversation-owned asynchronous execution by its explicit \
             execution handle (kind + id). The handle is returned by the tool call that \
             created the execution: a detached background tool execution has kind \"tool\", \
             and an asynchronous subagent child has kind \"subagent\". Pass the exact handle \
             from the creation result; the kind is never guessed from the id."
                .to_owned(),
        input_schema: input_schema::<ExecutionInput>(),
        execution_policy: ToolExecutionPolicy::ForegroundOnly,
        concurrency_policy: ToolConcurrencyPolicy::Sequential,
        approval_policy: crate::tools::types::ToolApprovalPolicy::Never,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Builtin,
    }
}

/// The executor of the `execution` intrinsic.
///
/// The executor holds handles to the conversation-owned domain registries;
/// lookup is scoped to that conversation by construction, so another
/// conversation's execution id is indistinguishable from an unknown id. It
/// never reaches around a registry: subagent cancellation goes through
/// `SubagentRegistry` (the logical lifecycle/cancellation authority), which
/// alone owns the child process-driver handoff.
pub struct ExecutionExecutor {
    background: ConversationBackgroundRegistry,
    subagents: Option<SubagentRegistry>,
}

impl ExecutionExecutor {
    /// Creates the intrinsic executor over the conversation-owned domain
    /// registries.
    #[must_use]
    pub fn new(
        background: ConversationBackgroundRegistry,
        subagents: Option<SubagentRegistry>,
    ) -> Self {
        Self {
            background,
            subagents,
        }
    }
}

impl ToolExecutor for ExecutionExecutor {
    fn execute<'a>(
        &'a self,
        invocation: ToolInvocation,
        _context: ToolExecutionContext<'a>,
    ) -> BoxFuture<'a, ToolExecutionResult> {
        let background = self.background.clone();
        let subagents = self.subagents.clone();
        Box::pin(
            async move { run_execution(&background, subagents.as_ref(), &invocation.arguments) },
        )
    }
}

/// Runs one `execution` invocation against the owning domain registry.
fn run_execution(
    background: &ConversationBackgroundRegistry,
    subagents: Option<&SubagentRegistry>,
    arguments: &serde_json::Value,
) -> ToolExecutionResult {
    let input = match ExecutionInput::parse(arguments) {
        Ok(input) => input,
        Err(error) => return failed(error),
    };
    let id = input.target.id;
    match input.target.kind {
        // Detached tool executions are owned by the conversation background
        // registry; a mismatched kind/id pair is exactly an unknown id
        // there, never a fallback to another domain.
        ExecutionKind::Tool => {
            let execution_id = ToolExecutionId::new(&id);
            let snapshot = match input.action {
                ExecutionAction::Status => background.snapshot(&execution_id),
                ExecutionAction::Cancel => background.cancel(&execution_id),
            };
            match snapshot {
                Some(snapshot) => snapshot_result(ExecutionSnapshot::Tool { snapshot }),
                None => failed(format!("unknown background execution {id}")),
            }
        }
        // Subagent children are owned by the conversation's subagent
        // registry, which remains the sole logical lifecycle/cancellation
        // authority. The intrinsic never manipulates child/process handles
        // directly.
        ExecutionKind::Subagent => {
            let Some(subagents) = subagents else {
                return failed(format!("unknown subagent execution {id}"));
            };
            let subagent_id = SubagentId::new(&id);
            let snapshot = match input.action {
                ExecutionAction::Status => subagents.snapshot(&subagent_id),
                ExecutionAction::Cancel => {
                    subagents.cancel(&subagent_id, CancellationReason::UserRequested)
                }
            };
            match snapshot {
                Some(snapshot) => snapshot_result(ExecutionSnapshot::Subagent { snapshot }),
                None => failed(format!("unknown subagent execution {id}")),
            }
        }
    }
}

/// The bounded tagged model-facing response of one `execution` call.
///
/// The outer envelope carries the explicit kind; the domain snapshot's own
/// fields are preserved verbatim, so no lifecycle semantics are erased. The
/// intrinsic owns this envelope only — the state it projects is always the
/// owning registry's authoritative snapshot.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExecutionSnapshot {
    /// The authoritative snapshot of one detached tool execution.
    Tool {
        #[serde(flatten)]
        snapshot: BackgroundExecutionSnapshot,
    },
    /// The authoritative snapshot of one subagent child.
    Subagent {
        #[serde(flatten)]
        snapshot: crate::runtime::subagent::SubagentSnapshot,
    },
}

impl ExecutionSnapshot {}

/// The canonical snapshot result: the full bounded tagged snapshot.
fn snapshot_result(snapshot: ExecutionSnapshot) -> ToolExecutionResult {
    ToolExecutionResult {
        status: ToolExecutionStatus::Success,
        content: vec![ToolResultContent::Json {
            value: serde_json::to_value(snapshot).expect("execution snapshots serialize"),
        }],
        duration_ms: 0,
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
        managed_output: None,
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
        managed_output: None,
    }
}

#[cfg(test)]
mod tests {
    use super::{ExecutionAction, ExecutionInput, ExecutionKind, ExecutionSnapshot};
    use crate::runtime::identity::ToolExecutionId;
    use crate::tools::background::BackgroundExecutionSnapshot;

    #[test]
    fn the_input_contract_requires_an_explicit_tagged_target() {
        let parsed = ExecutionInput::parse(&serde_json::json!({
            "action": "status",
            "target": {"kind": "tool", "id": "exec_1"},
        }))
        .expect("the canonical contract parses");
        assert_eq!(parsed.action, ExecutionAction::Status);
        assert_eq!(parsed.target.kind, ExecutionKind::Tool);
        assert_eq!(parsed.target.id, "exec_1");

        for missing in [
            serde_json::json!({"action": "status"}),
            serde_json::json!({"target": {"kind": "tool", "id": "exec_1"}}),
            serde_json::json!({"action": "status", "target": {"kind": "tool"}}),
            serde_json::json!({"action": "status", "target": {"id": "exec_1"}}),
        ] {
            assert!(
                ExecutionInput::parse(&missing).is_err(),
                "a partial contract is rejected: {missing}"
            );
        }
        assert!(
            ExecutionInput::parse(&serde_json::json!({
                "action": "status",
                "target": {"kind": "tool", "id": "exec_1"},
                "extra": true,
            }))
            .is_err(),
            "unknown fields are rejected"
        );
    }

    #[test]
    fn the_action_set_is_closed() {
        for action in ["status", "cancel"] {
            assert!(
                ExecutionInput::parse(&serde_json::json!({
                    "action": action,
                    "target": {"kind": "tool", "id": "exec_1"},
                }))
                .is_ok(),
                "{action} is a legal action"
            );
        }
        for action in [
            "wait",
            "delete",
            "restart",
            "list",
            "schedule",
            "poll_result",
        ] {
            let rejected = ExecutionInput::parse(&serde_json::json!({
                "action": action,
                "target": {"kind": "tool", "id": "exec_1"},
            }))
            .expect_err("outside the closed action set");
            assert!(
                rejected.contains("status") && rejected.contains("cancel"),
                "the rejection names the closed action set: {rejected}"
            );
        }
    }

    #[test]
    fn the_kind_set_is_closed() {
        for kind in ["tool", "subagent"] {
            assert!(
                ExecutionInput::parse(&serde_json::json!({
                    "action": "status",
                    "target": {"kind": kind, "id": "x"},
                }))
                .is_ok(),
                "{kind} is a legal kind"
            );
        }
        assert!(
            ExecutionInput::parse(&serde_json::json!({
                "action": "status",
                "target": {"kind": "task", "id": "x"},
            }))
            .is_err(),
            "an unknown kind is a contract violation, never a guessed route"
        );
    }

    #[test]
    fn the_generated_schema_is_the_closed_bounded_contract() {
        let schema = crate::tools::native::registration::input_schema::<ExecutionInput>();
        let properties = schema["properties"].as_object().expect("properties");
        let mut names = properties.keys().cloned().collect::<Vec<_>>();
        names.sort();
        assert_eq!(names, vec!["action", "target"]);
        assert_eq!(
            properties["action"]["enum"],
            serde_json::json!(["status", "cancel"])
        );
        assert_eq!(
            properties["target"]["properties"]["kind"]["enum"],
            serde_json::json!(["tool", "subagent"])
        );
        assert!(
            properties["target"]["required"]
                .as_array()
                .expect("required")
                .contains(&serde_json::json!("id"))
        );
    }

    #[test]
    fn the_response_envelope_is_tagged_and_preserves_domain_fields() {
        let snapshot = BackgroundExecutionSnapshot {
            execution_id: ToolExecutionId::new("exec_1"),
            tool_id: crate::runtime::identity::ToolId::new("tool-bash"),
            tool_name: "bash".to_owned(),
            state: crate::tools::background::BackgroundLifecycle::Running,
            progress: None,
            result: None,
        };
        let value = serde_json::to_value(ExecutionSnapshot::Tool { snapshot }).expect("serializes");
        assert_eq!(value["kind"], "tool");
        assert_eq!(value["execution_id"], "exec_1");
        assert_eq!(value["tool_name"], "bash");
        assert_eq!(value["state"], "running");
    }
}
