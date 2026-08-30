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
//! channel: the subagent response is a bounded [`SubagentExecutionSnapshot`]
//! projection that deliberately excludes the registry's internal terminal
//! `detail` (which carries the successful child answer), so a subagent's
//! final answer still arrives exactly once through the existing canonical
//! inbound message path — never through `execution`.
//!
//! The intrinsic's policies are fixed to foreground-only sequential
//! execution and it may never become background-dispatchable (enforced by
//! the registry).

mod input;

use futures_util::future::BoxFuture;

use chrono::{DateTime, Utc};

use crate::runtime::identity::{AgentId, ConversationId, SubagentId, ToolCallId, ToolExecutionId};
use crate::runtime::subagent::{
    SubagentRegistry, SubagentSnapshot, SubagentState, WorkspaceHandoff, WorkspaceSnapshot,
};
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
                Some(snapshot) => snapshot_result(ExecutionSnapshot::Subagent {
                    snapshot: snapshot.into(),
                }),
                None => failed(format!("unknown subagent execution {id}")),
            }
        }
    }
}

/// The bounded model-facing projection of one subagent child execution
/// (Issue #162).
///
/// Derived from the registry's authoritative [`SubagentSnapshot`] at
/// response time — it is a projection of the registry's read model, never
/// an authority of its own and never a second lifecycle record.
///
/// The projection exposes lifecycle/identity/control facts only. It
/// deliberately excludes the registry's internal `detail` field, which the
/// subagent settlement path populates with the successful child answer
/// content: the model-facing control plane must never carry the child's
/// answer, so the canonical inbound child-agent message stays the **only**
/// result-delivery channel and `execution(status|cancel)` stays pure
/// lifecycle observation/control. The same guarantee holds while the
/// registry is still in `PublishingTerminal`: the pending answer is never
/// model-visible through the intrinsic.
///
/// [`SubagentSnapshot`]: crate::runtime::subagent::SubagentSnapshot
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SubagentExecutionSnapshot {
    /// The conversation-owned subagent identity.
    pub subagent_id: SubagentId,
    /// The child agent identity (provenance of its answer).
    pub child_agent_id: AgentId,
    /// The child's own durable conversation identity.
    pub child_conversation_id: ConversationId,
    /// The delegating tool call.
    pub tool_call_id: ToolCallId,
    /// The canonical named-agent identity frozen at start (Issue #144).
    pub agent: String,
    /// The deterministic definition digest frozen at start (Issue #144).
    pub definition_digest: String,
    /// The immutable project-workspace authority selected before ownership.
    pub workspace: WorkspaceSnapshot,
    /// Retained work-product metadata, when terminal settlement preserves an
    /// isolated worktree for handoff.
    pub handoff: Option<WorkspaceHandoff>,
    /// The lifecycle state.
    pub state: SubagentState,
    /// Whether a terminal publication could not reach the durable
    /// authority and was abandoned.
    pub publication_abandoned: bool,
    /// Whether the child reached a settled state (terminal, publication
    /// not abandoned).
    pub settled: bool,
    /// When the ownership committed.
    pub started_at: DateTime<Utc>,
}

impl From<SubagentSnapshot> for SubagentExecutionSnapshot {
    fn from(snapshot: SubagentSnapshot) -> Self {
        // The registry's authoritative snapshot is projected field by
        // field. `detail` — the registry-internal terminal detail that
        // carries the successful child answer — is intentionally dropped:
        // it is domain-internal state, never model-facing result content.
        let SubagentSnapshot {
            subagent_id,
            child_agent_id,
            child_conversation_id,
            tool_call_id,
            agent,
            definition_digest,
            workspace,
            handoff,
            state,
            detail: _,
            publication_abandoned,
            settled,
            started_at,
        } = snapshot;
        Self {
            subagent_id,
            child_agent_id,
            child_conversation_id,
            tool_call_id,
            agent,
            definition_digest,
            workspace,
            handoff,
            state,
            publication_abandoned,
            settled,
            started_at,
        }
    }
}

/// The bounded tagged model-facing response of one `execution` call.
///
/// The outer envelope carries the explicit kind. The tool variant carries
/// the authoritative `BackgroundExecutionSnapshot`; the subagent variant
/// carries the bounded [`SubagentExecutionSnapshot`] projection derived
/// from the registry's authoritative snapshot. No lifecycle semantics are
/// erased, and no result payload is introduced: the intrinsic owns this
/// envelope only — the state it projects is always the owning registry's
/// authoritative snapshot.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExecutionSnapshot {
    /// The authoritative snapshot of one detached tool execution.
    Tool {
        #[serde(flatten)]
        snapshot: BackgroundExecutionSnapshot,
    },
    /// The authoritative snapshot of one subagent child, projected into
    /// the bounded model-facing [`SubagentExecutionSnapshot`].
    Subagent {
        #[serde(flatten)]
        snapshot: SubagentExecutionSnapshot,
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

    /// The subagent projection keeps every lifecycle/identity/control fact
    /// but can never expose the registry-internal terminal `detail` that
    /// carries the successful child answer.
    #[test]
    fn the_subagent_projection_never_exposes_the_child_answer() {
        use crate::runtime::identity::{AgentId, ConversationId, SubagentId, ToolCallId};
        use crate::runtime::subagent::{SubagentSnapshot, SubagentState, WorkspaceSnapshot};
        let snapshot = SubagentSnapshot {
            subagent_id: SubagentId::new("conversation-1-subagent-2"),
            child_agent_id: AgentId::new("agent-child"),
            child_conversation_id: ConversationId::new("conversation-1-subagent-2"),
            tool_call_id: ToolCallId::new("call-1"),
            agent: "explore".to_owned(),
            definition_digest: "sha256:d1".to_owned(),
            workspace: WorkspaceSnapshot::shared(std::path::PathBuf::from("<shared-workspace>")),
            handoff: None,
            state: SubagentState::Succeeded,
            // The registry-internal terminal detail carries the successful
            // child answer; the projection must drop it.
            detail: Some("issue162-secret-child-answer".to_owned()),
            publication_abandoned: false,
            settled: true,
            started_at: chrono::Utc::now(),
        };
        let projection: super::SubagentExecutionSnapshot = snapshot.clone().into();
        assert_eq!(projection.subagent_id, snapshot.subagent_id);
        assert_eq!(projection.child_agent_id, snapshot.child_agent_id);
        assert_eq!(projection.agent, "explore");
        assert_eq!(projection.state, SubagentState::Succeeded);
        assert!(projection.settled);

        let value = serde_json::to_value(projection).expect("serializes");
        assert_eq!(value["subagent_id"], "conversation-1-subagent-2");
        assert_eq!(value["state"], "succeeded");
        assert!(
            value.get("detail").is_none(),
            "detail is not a model-facing field: {value}"
        );
        let serialized = serde_json::to_string(&value).expect("string");
        assert!(
            !serialized.contains("issue162-secret-child-answer"),
            "the child answer never appears in the projection: {serialized}"
        );
    }
}
