//! The `execution` runtime intrinsic (Issue #162, discovery from #180).
//!
//! `execution` is the single model-facing observation, discovery, and
//! cancellation control plane for conversation-owned asynchronous
//! executions. It is a **control-plane router only**: it owns model-facing
//! schema/input validation, explicit target-kind dispatch, conversion into
//! the owning domain's id type, invocation of the owning registry API, and
//! conversion of the authoritative domain snapshots into bounded tagged
//! model-facing responses.
//!
//! It owns **no lifecycle state, no task/process handles, no cancellation
//! tokens, no cancellation implementation, no durability, no terminal
//! settlement, no result publication, no registry, and no cache**. Every
//! request is routed to the domain authority that owns the execution:
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
//!   | authoritative listing       | authoritative listing
//!   v                             v
//! BackgroundExecutionSnapshot     SubagentSnapshot
//! BackgroundExecutionListing      SubagentListing
//! ```
//!
//! Each domain owns its own read models, including its own bounded
//! discovery listing; this intrinsic consumes them. The dependency runs one
//! way only — the model-facing control plane knows both domain authorities,
//! and neither domain authority knows this control plane or the shared
//! `tools::execution` envelope's response bound.
//!
//! The canonical input is **action-tagged**: the action selects which
//! fields exist at all, so a target-less action cannot be spelled with a
//! target and a targeted action cannot be spelled without one.
//!
//! ```json
//! {"action": "status", "target": {"kind": "tool | subagent", "id": "..."}}
//! {"action": "cancel", "target": {"kind": "tool | subagent", "id": "..."}}
//! {"action": "list",   "filter": {"kind": "tool | subagent", "active_only": true}}
//! ```
//!
//! The target kind is explicit and closed. The intrinsic never infers a
//! kind from an id prefix and never tries one registry and falls through to
//! another: a mismatched kind/id pair fails through the selected domain
//! authority exactly like an unknown id, and cross-conversation ids remain
//! indistinguishable from unknown ids at the owning domain boundary. The
//! `list` filter's kind selects which authority is consulted at all, so it
//! cannot fall through either.
//!
//! `status` is single-target observation, `list` is bounded discovery, and
//! `cancel` is control. **None of them is a result channel.** The subagent
//! status response is a bounded [`SubagentExecutionSnapshot`] projection
//! and a listing entry is a narrower [`ExecutionSummary`] still; both
//! deliberately exclude the registry's internal `detail` (Issue #178:
//! diagnostics only, never the answer) and the live observation-plane
//! fields, and a listing additionally excludes the detached tool
//! execution's own `result` and `progress`. A subagent's final answer
//! arrives exactly once through the existing canonical inbound message
//! path — never through `execution` — and a detached tool execution's
//! output stays on its own domain channel.
//!
//! # Discovery: ordering, bound, and scope (Issue #180)
//!
//! Discovery is conversation-scoped **by construction**: the executor holds
//! the registries this conversation owns, so a foreign execution is not
//! filtered out — it is unreachable, and indistinguishable from absence.
//!
//! Each domain produces its own bounded authoritative listing, most
//! recently allocated first *within that domain*. The intrinsic merges the
//! two by strict alternation starting with the tool domain and truncates
//! the result to the single global [`MAX_LISTED_EXECUTIONS`] bound,
//! reporting `returned`, `matched`, `truncated`, and `limit` explicitly.
//! The two domains allocate from independent sequences, so no shared
//! ordinal could interleave them; alternation is what keeps one domain's
//! overflow from starving the other out of a single global bound.
//!
//! The merged sequence is therefore deterministic but deliberately **not**
//! globally most-recent-first: newest-first holds inside each domain only,
//! and no cross-domain chronological claim is made — or could be made,
//! since the domains share no ordinal or clock. The model-facing tool
//! description says exactly this.
//!
//! The intrinsic's policies are fixed to foreground-only sequential
//! execution and it may never become background-dispatchable (enforced by
//! the registry).

mod input;

use futures_util::future::BoxFuture;

use chrono::{DateTime, Utc};

use crate::runtime::identity::{AgentId, ConversationId, SubagentId, ToolCallId, ToolExecutionId};
use crate::runtime::subagent::{
    SubagentListing, SubagentRegistry, SubagentSnapshot, SubagentState, WorkspaceHandoff,
    WorkspaceSnapshot,
};
use crate::runtime::types::CancellationReason;
use crate::tools::background::{
    BackgroundExecutionListing, BackgroundExecutionSnapshot, BackgroundLifecycle,
    ConversationBackgroundRegistry,
};
use crate::tools::execution::{ExecutionHandle, ExecutionKind, MAX_LISTED_EXECUTIONS};
use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::native::registration::{NativeToolRegistration, input_schema};
use crate::tools::types::{
    ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolExecutionResult,
    ToolExecutionStatus, ToolInvocation, ToolOrigin, ToolReplayPolicy, ToolResultContent,
};

use input::{ExecutionFilter, ExecutionInput};

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
        description: "Inspect, cancel, or list this conversation's asynchronous executions. \
             \"status\" and \"cancel\" name one execution by its explicit execution handle \
             (kind + id) as returned by the tool call that created it: a detached \
             background tool execution has kind \"tool\", an asynchronous subagent child \
             has kind \"subagent\". Pass the exact handle from the creation result; the \
             kind is never guessed from the id. \"list\" takes no target and returns a \
             bounded, deterministically ordered summary of this conversation's own \
             executions — newest-first within each execution kind, the kinds interleaved — \
             optionally filtered by kind and to lifecycle-active ones; it reports handles \
             and lifecycle state only, never execution output, a subagent's answer, or a \
             child's history."
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
    match ExecutionInput::parse(arguments) {
        Ok(ExecutionInput::Status { target }) => run_status(background, subagents, &target),
        Ok(ExecutionInput::Cancel { target }) => run_cancel(background, subagents, &target),
        Ok(ExecutionInput::List { filter }) => run_list(background, subagents, filter),
        Err(error) => failed(error),
    }
}

/// Observes one execution through its owning domain authority.
fn run_status(
    background: &ConversationBackgroundRegistry,
    subagents: Option<&SubagentRegistry>,
    target: &ExecutionHandle,
) -> ToolExecutionResult {
    match target.kind {
        // Detached tool executions are owned by the conversation background
        // registry; a mismatched kind/id pair is exactly an unknown id
        // there, never a fallback to another domain.
        ExecutionKind::Tool => {
            let snapshot = background.snapshot(&ToolExecutionId::new(&target.id));
            tool_snapshot_result(snapshot, &target.id)
        }
        // Subagent children are owned by the conversation's subagent
        // registry, which remains the sole logical lifecycle/cancellation
        // authority. The intrinsic never manipulates child/process handles
        // directly.
        ExecutionKind::Subagent => {
            let snapshot =
                subagents.and_then(|subagents| subagents.snapshot(&SubagentId::new(&target.id)));
            subagent_snapshot_result(snapshot, &target.id)
        }
    }
}

/// Requests cancellation of one execution through its owning domain
/// authority, which alone implements it.
fn run_cancel(
    background: &ConversationBackgroundRegistry,
    subagents: Option<&SubagentRegistry>,
    target: &ExecutionHandle,
) -> ToolExecutionResult {
    match target.kind {
        ExecutionKind::Tool => {
            let snapshot = background.cancel(&ToolExecutionId::new(&target.id));
            tool_snapshot_result(snapshot, &target.id)
        }
        ExecutionKind::Subagent => {
            let snapshot = subagents.and_then(|subagents| {
                subagents.cancel(
                    &SubagentId::new(&target.id),
                    CancellationReason::UserRequested,
                )
            });
            subagent_snapshot_result(snapshot, &target.id)
        }
    }
}

/// Discovers this conversation's own executions (Issue #180).
///
/// Discovery is conversation-scoped **by construction**, not by filtering:
/// the executor holds the two registries this conversation owns, so there is
/// no wider set to scope down and a foreign execution is not merely hidden —
/// it is unreachable. A runtime that owns no subagent registry has no
/// subagents to list, exactly as it has none to name.
///
/// The kind filter selects which domain authority is consulted at all: with
/// `kind = tool` the subagent registry is never asked, and with
/// `kind = subagent` the background registry is never asked, so a filter can
/// never fall through into the other domain.
///
/// Ordering and bounding are runtime-owned, never caller-selectable:
///
/// - each domain returns its matching records **most recently allocated
///   first**, in its own authoritative allocation order;
/// - the two domain sequences are merged by strict alternation starting with
///   the tool domain (tool, subagent, tool, subagent, …); when one sequence
///   runs out, the remainder of the other follows in order;
/// - the merged sequence is truncated to [`MAX_LISTED_EXECUTIONS`].
///
/// Alternation exists because the two domains allocate from independent
/// sequences, so no shared clock or ordinal can order them against each
/// other; concatenating the domains would have been equally deterministic
/// but would let one domain's overflow starve the other out of the bounded
/// response entirely. Alternation keeps the bound a single global number
/// while guaranteeing both domains are represented.
fn run_list(
    background: &ConversationBackgroundRegistry,
    subagents: Option<&SubagentRegistry>,
    filter: ExecutionFilter,
) -> ToolExecutionResult {
    let active_only = filter.active_only();
    // Each domain is asked for at most the *global* bound: the per-domain
    // request is only an upper bound on materialization, never a per-domain
    // quota, so the externally visible bound stays exactly one number.
    let tools = match filter.kind {
        None | Some(ExecutionKind::Tool) => background.listing(active_only, MAX_LISTED_EXECUTIONS),
        // A domain this request never consults contributes the empty
        // listing: it is not asked and then filtered, it is not asked.
        Some(ExecutionKind::Subagent) => BackgroundExecutionListing {
            snapshots: Vec::new(),
            matched: 0,
        },
    };
    let children = match (filter.kind, subagents) {
        // Likewise for the subagent domain — and a runtime that owns no
        // subagent registry has no children to list at all.
        (Some(ExecutionKind::Tool), _) | (_, None) => SubagentListing {
            snapshots: Vec::new(),
            matched: 0,
        },
        (None | Some(ExecutionKind::Subagent), Some(subagents)) => {
            subagents.listing(active_only, MAX_LISTED_EXECUTIONS)
        }
    };

    json_result(&merge_bounded(tools, children))
}

/// Merges the two domain listings into the bounded model-facing response.
///
/// This is the whole ordering and bounding contract, and it is a pure
/// function of the two domain listings: given the same listings it returns
/// the same entries and the same counts, so repeating a request against
/// unchanged registries is stable by construction.
fn merge_bounded(
    tools: BackgroundExecutionListing,
    children: SubagentListing,
) -> ExecutionListingResponse {
    let matched = tools.matched + children.matched;
    let mut executions = Vec::with_capacity(tools.snapshots.len() + children.snapshots.len());
    let mut tools = tools.snapshots.into_iter();
    let mut children = children.snapshots.into_iter();
    // Strict alternation, tool first, until both domains are exhausted: the
    // two domains have no common ordinal to interleave by, and alternation
    // keeps a single global bound from starving either of them.
    loop {
        let tool = tools.next().map(ExecutionSummary::of_tool);
        let child = children.next().map(ExecutionSummary::of_subagent);
        if tool.is_none() && child.is_none() {
            break;
        }
        executions.extend(tool);
        executions.extend(child);
    }
    executions.truncate(MAX_LISTED_EXECUTIONS);
    ExecutionListingResponse {
        returned: executions.len(),
        matched,
        truncated: matched > executions.len(),
        limit: MAX_LISTED_EXECUTIONS,
        executions,
    }
}

/// The canonical response of a `kind = tool` target operation.
fn tool_snapshot_result(
    snapshot: Option<BackgroundExecutionSnapshot>,
    id: &str,
) -> ToolExecutionResult {
    match snapshot {
        Some(snapshot) => json_result(&ExecutionSnapshot::Tool { snapshot }),
        None => failed(format!("unknown background execution {id}")),
    }
}

/// The canonical response of a `kind = subagent` target operation.
fn subagent_snapshot_result(snapshot: Option<SubagentSnapshot>, id: &str) -> ToolExecutionResult {
    match snapshot {
        Some(snapshot) => json_result(&ExecutionSnapshot::Subagent {
            snapshot: snapshot.into(),
        }),
        None => failed(format!("unknown subagent execution {id}")),
    }
}

/// The bounded model-facing response of `execution(list)` (Issue #180).
///
/// The counts are the truncation contract and are always present, so the
/// response shape never changes with the data: `returned` is how many
/// entries the array carries, `matched` how many executions matched the
/// filter across both domains before the bound was applied, `truncated`
/// whether the bound held any back, and `limit` the bound itself. Repeating
/// an identical request against unchanged registries returns identical
/// entries and identical counts.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ExecutionListingResponse {
    /// The bounded, deterministically ordered execution summaries.
    pub executions: Vec<ExecutionSummary>,
    /// How many summaries this response carries.
    pub returned: usize,
    /// How many executions matched the filter before the bound.
    pub matched: usize,
    /// Whether the bound held matching executions back.
    pub truncated: bool,
    /// The global response bound.
    pub limit: usize,
}

/// One bounded model-facing execution summary (Issue #180).
///
/// A summary is a *discovery* read model, deliberately much smaller than the
/// single-target [`ExecutionSnapshot`]: it carries the typed
/// [`ExecutionHandle`] needed to act on the execution, the owning domain's
/// own lifecycle state, and the few identity facts that make an entry
/// recognizable. The handle is always explicit and always the domain's own,
/// so a caller never has to infer a kind from an id shape.
///
/// `state` is deliberately the **owning domain's** state vocabulary rather
/// than a unified one: unifying would have created a second lifecycle
/// vocabulary owned by `execution`, and `list` and `status` must project the
/// same lifecycle facts for the same execution.
///
/// What a summary never carries is as much of the contract as what it does:
///
/// - no tool `result` and no tool `progress`, so a listing can never become
///   a second delivery path for detached tool output;
/// - no subagent `detail`, so a failure diagnostic stays on the single
///   target status surface;
/// - no subagent answer content and no child history, so the canonical
///   inbound child-agent message remains the one result channel;
/// - no observation-plane field (Issue #178's live `activity`,
///   `last_activity_at`, counters, or execution `profile`), because that
///   plane is deliberately not model-facing: `execution(status)` already
///   drops it, and a listing that carried it would have made the same
///   intrinsic expose through one action what it hides through another.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(untagged)]
pub enum ExecutionSummary {
    /// One detached tool execution.
    Tool {
        /// The typed handle of the execution.
        execution: ExecutionHandle,
        /// The owning registry's authoritative lifecycle state.
        state: BackgroundLifecycle,
        /// The model-facing name of the executing tool.
        tool_name: String,
    },
    /// One asynchronous subagent child.
    Subagent {
        /// The typed handle of the execution.
        execution: ExecutionHandle,
        /// The owning registry's authoritative lifecycle state.
        state: SubagentState,
        /// The canonical named-agent identity frozen at start.
        agent: String,
        /// When the ownership committed.
        started_at: DateTime<Utc>,
        /// Whether a terminal publication was abandoned, which is what
        /// distinguishes a child still settling from one that can never
        /// settle.
        publication_abandoned: bool,
    },
}

impl ExecutionSummary {
    /// Projects one authoritative background snapshot into a summary.
    fn of_tool(snapshot: BackgroundExecutionSnapshot) -> Self {
        // `progress` and `result` are dropped: a listing is discovery, not a
        // second output channel for detached tool executions.
        Self::Tool {
            execution: ExecutionHandle::tool(&snapshot.execution_id),
            state: snapshot.state,
            tool_name: snapshot.tool_name,
        }
    }

    /// Projects one authoritative subagent snapshot into a summary.
    fn of_subagent(snapshot: SubagentSnapshot) -> Self {
        // `detail`, `observation`, and `profile` are dropped; see the type
        // documentation for why each one is not model-facing here.
        Self::Subagent {
            execution: ExecutionHandle::subagent(&snapshot.subagent_id),
            state: snapshot.state,
            agent: snapshot.agent,
            started_at: snapshot.started_at,
            publication_abandoned: snapshot.publication_abandoned,
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
/// deliberately excludes the registry's internal `detail` field (a
/// failure/cancellation diagnostic; Issue #178 removed the successful
/// answer content from it entirely) and the live observation-plane fields:
/// the model-facing control plane must never carry the child's answer or
/// its live activity, so the canonical inbound child-agent message stays
/// the **only** result-delivery channel and `execution(status|cancel)`
/// stays pure lifecycle observation/control. The same guarantee holds
/// while the registry is still in `PublishingTerminal`: the pending answer
/// is never model-visible through the intrinsic.
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
        // field. `detail` — the registry-internal terminal diagnostic — is
        // intentionally dropped, as are the observation-plane `observation`
        // and `profile` fields (Issue #178): live activity enters no model
        // context, and the canonical inbound child-agent message stays the
        // only result-delivery channel.
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
            observation: _,
            profile: _,
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

/// The canonical successful result: one bounded JSON model-facing value.
fn json_result(value: &impl serde::Serialize) -> ToolExecutionResult {
    ToolExecutionResult {
        status: ToolExecutionStatus::Success,
        content: vec![ToolResultContent::Json {
            value: serde_json::to_value(value).expect("execution responses serialize"),
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
    use chrono::{DateTime, Utc};

    use super::{
        ExecutionInput, ExecutionKind, ExecutionSnapshot, ExecutionSummary, MAX_LISTED_EXECUTIONS,
    };
    use crate::runtime::identity::ToolExecutionId;
    use crate::runtime::subagent::{SubagentListing, SubagentSnapshot};
    use crate::tools::background::{BackgroundExecutionListing, BackgroundExecutionSnapshot};

    /// Every legal invocation of the action-tagged contract parses, and the
    /// action determines which fields exist at all.
    #[test]
    fn the_input_contract_is_action_tagged() {
        assert!(matches!(
            ExecutionInput::parse(&serde_json::json!({
                "action": "status",
                "target": {"kind": "tool", "id": "exec_1"},
            }))
            .expect("status parses"),
            ExecutionInput::Status { target } if target.kind == ExecutionKind::Tool && target.id == "exec_1"
        ));
        assert!(matches!(
            ExecutionInput::parse(&serde_json::json!({
                "action": "cancel",
                "target": {"kind": "subagent", "id": "c-1-subagent-2"},
            }))
            .expect("cancel parses"),
            ExecutionInput::Cancel { target } if target.kind == ExecutionKind::Subagent
        ));
        let ExecutionInput::List { filter } = ExecutionInput::parse(&serde_json::json!({
            "action": "list",
        }))
        .expect("a bare list parses") else {
            panic!("list parses as list");
        };
        assert_eq!(filter.kind, None, "an omitted filter constrains nothing");
        assert_eq!(filter.active_only, None);
        assert!(
            !filter.active_only(),
            "the default lists terminal records too"
        );

        let ExecutionInput::List { filter } = ExecutionInput::parse(&serde_json::json!({
            "action": "list",
            "filter": {"kind": "subagent", "active_only": true},
        }))
        .expect("a filtered list parses") else {
            panic!("list parses as list");
        };
        assert_eq!(filter.kind, Some(ExecutionKind::Subagent));
        assert!(filter.active_only());
    }

    /// The tagged union makes every ill-formed action/field combination a
    /// schema violation rather than a runtime special case.
    #[test]
    fn the_contract_rejects_every_mismatched_action_shape() {
        for rejected in [
            // `status`/`cancel` name exactly one execution.
            serde_json::json!({"action": "status"}),
            serde_json::json!({"action": "cancel"}),
            serde_json::json!({"action": "status", "target": {"kind": "tool"}}),
            serde_json::json!({"action": "status", "target": {"id": "exec_1"}}),
            // `list` names none: a target is not an ignored field.
            serde_json::json!({"action": "list", "target": {"kind": "tool", "id": "exec_1"}}),
            // A filter belongs to `list` alone.
            serde_json::json!({
                "action": "status",
                "target": {"kind": "tool", "id": "exec_1"},
                "filter": {"kind": "tool"},
            }),
            // The action itself is required.
            serde_json::json!({"target": {"kind": "tool", "id": "exec_1"}}),
            serde_json::json!({"filter": {"kind": "tool"}}),
            // Unknown fields are rejected at every level.
            serde_json::json!({
                "action": "status",
                "target": {"kind": "tool", "id": "exec_1"},
                "extra": true,
            }),
            serde_json::json!({"action": "list", "filter": {"kind": "tool", "extra": true}}),
            serde_json::json!({"action": "list", "extra": true}),
            // Malformed filter values.
            serde_json::json!({"action": "list", "filter": {"kind": "task"}}),
            serde_json::json!({"action": "list", "filter": {"active_only": "yes"}}),
        ] {
            assert!(
                ExecutionInput::parse(&rejected).is_err(),
                "an ill-formed invocation is a contract violation: {rejected}"
            );
        }
    }

    /// Preflight validates every invocation against the canonical schema
    /// before the intrinsic ever decodes it, so the schema itself — not a
    /// runtime special case — is what closes the shapes serde alone would
    /// still accept, and an optional property means an *absent* property
    /// rather than an explicit `null`.
    #[test]
    fn the_canonical_schema_rejects_what_serde_alone_would_accept() {
        let schema = crate::tools::native::registration::input_schema::<ExecutionInput>();
        for rejected in [
            // serde deserializes a struct from a sequence; the schema does
            // not.
            serde_json::json!({"action": "list", "filter": []}),
            // `null` is not a second spelling of omission.
            serde_json::json!({"action": "list", "filter": {"kind": null}}),
            serde_json::json!({"action": "list", "filter": {"active_only": null}}),
            serde_json::json!({"action": "list", "filter": null}),
        ] {
            assert!(
                crate::tools::schema::validate_business_arguments(&schema, &rejected).is_err(),
                "preflight rejects the ambiguous spelling: {rejected}"
            );
        }
        for accepted in [
            serde_json::json!({"action": "list"}),
            serde_json::json!({"action": "list", "filter": {}}),
            serde_json::json!({"action": "list", "filter": {"kind": "tool"}}),
            serde_json::json!({"action": "list", "filter": {"active_only": true}}),
            serde_json::json!({"action": "status", "target": {"kind": "tool", "id": "e"}}),
        ] {
            crate::tools::schema::validate_business_arguments(&schema, &accepted)
                .unwrap_or_else(|error| panic!("preflight accepts {accepted}: {error}"));
        }
    }

    /// The obsolete Issue #162 shape — one flat `action` beside a mandatory
    /// `target` — is explicitly rejected rather than silently accepted for
    /// the target-less action.
    #[test]
    fn the_obsolete_untagged_target_shape_is_rejected() {
        assert!(
            ExecutionInput::parse(&serde_json::json!({
                "action": "list",
                "target": {"kind": "subagent", "id": "c-1-subagent-2"},
            }))
            .is_err(),
            "a list may not carry the pre-#180 mandatory target"
        );
    }

    #[test]
    fn the_action_set_is_closed() {
        for (action, arguments) in [
            (
                "status",
                serde_json::json!({"action": "status", "target": {"kind": "tool", "id": "e"}}),
            ),
            (
                "cancel",
                serde_json::json!({"action": "cancel", "target": {"kind": "tool", "id": "e"}}),
            ),
            ("list", serde_json::json!({"action": "list"})),
        ] {
            assert!(
                ExecutionInput::parse(&arguments).is_ok(),
                "{action} is a legal action"
            );
        }
        for action in [
            "wait",
            "delete",
            "restart",
            "schedule",
            "poll_result",
            "output",
            "logs",
        ] {
            let rejected = ExecutionInput::parse(&serde_json::json!({
                "action": action,
                "target": {"kind": "tool", "id": "exec_1"},
            }))
            .expect_err("outside the closed action set");
            assert!(
                rejected.contains("status")
                    && rejected.contains("cancel")
                    && rejected.contains("list"),
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
            assert!(
                ExecutionInput::parse(&serde_json::json!({
                    "action": "list",
                    "filter": {"kind": kind},
                }))
                .is_ok(),
                "{kind} is a legal filter kind"
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

    /// The generated schema is a root object schema (the canonical tool
    /// schema policy) whose branches are the three closed actions.
    #[test]
    fn the_generated_schema_is_the_closed_bounded_contract() {
        let schema = crate::tools::native::registration::input_schema::<ExecutionInput>();
        crate::tools::schema::validate_canonical_schema(&schema)
            .expect("the intrinsic schema satisfies the canonical tool schema policy");
        assert_eq!(schema["type"], "object", "a root object schema: {schema}");
        let branches = schema["oneOf"].as_array().expect("one branch per action");
        assert_eq!(branches.len(), 3);

        let mut actions = Vec::new();
        for branch in branches {
            let properties = branch["properties"].as_object().expect("properties");
            let action = properties["action"]["const"]
                .as_str()
                .expect("each branch pins one action")
                .to_owned();
            let mut names = properties.keys().cloned().collect::<Vec<_>>();
            names.sort();
            match action.as_str() {
                "status" | "cancel" => {
                    assert_eq!(names, vec!["action", "target"]);
                    assert_eq!(
                        properties["target"]["properties"]["kind"]["enum"],
                        serde_json::json!(["tool", "subagent"])
                    );
                    assert!(
                        branch["required"]
                            .as_array()
                            .expect("required")
                            .contains(&serde_json::json!("target")),
                        "a target operation requires its target"
                    );
                }
                "list" => {
                    assert_eq!(names, vec!["action", "filter"]);
                    let filter = properties["filter"]["properties"]
                        .as_object()
                        .expect("filter properties");
                    let mut filter_names = filter.keys().cloned().collect::<Vec<_>>();
                    filter_names.sort();
                    assert_eq!(
                        filter_names,
                        vec!["active_only", "kind"],
                        "the filter vocabulary stays closed and minimal"
                    );
                    assert_eq!(
                        filter["kind"]["enum"],
                        serde_json::json!(["tool", "subagent"])
                    );
                }
                other => panic!("unexpected action branch {other}"),
            }
            actions.push(action);
        }
        actions.sort();
        assert_eq!(actions, vec!["cancel", "list", "status"]);
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
    /// but can never expose the registry-internal terminal `detail` or the
    /// live observation-plane fields.
    #[test]
    fn the_subagent_projection_never_exposes_the_child_answer() {
        let snapshot = subagent_snapshot();
        let projection: super::SubagentExecutionSnapshot = snapshot.clone().into();
        assert_eq!(projection.subagent_id, snapshot.subagent_id);
        assert_eq!(projection.child_agent_id, snapshot.child_agent_id);
        assert_eq!(projection.agent, "explore");
        assert_eq!(
            projection.state,
            crate::runtime::subagent::SubagentState::Succeeded
        );
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

    /// A tool summary is a discovery read model: the typed handle plus the
    /// owning registry's lifecycle state, never the detached execution's
    /// output.
    #[test]
    fn a_tool_summary_carries_the_handle_and_state_but_no_output() {
        let snapshot = BackgroundExecutionSnapshot {
            execution_id: ToolExecutionId::new("exec_7"),
            tool_id: crate::runtime::identity::ToolId::new("tool-bash"),
            tool_name: "bash".to_owned(),
            state: crate::tools::background::BackgroundLifecycle::Succeeded,
            progress: Some(crate::tools::types::ToolProgress {
                message: Some("issue180-secret-progress".to_owned()),
                completed: None,
                total: None,
            }),
            result: Some(crate::tools::types::ToolExecutionResult {
                status: crate::tools::types::ToolExecutionStatus::Success,
                content: vec![crate::tools::types::ToolResultContent::Text(
                    crate::message::content::TextBlock {
                        text: "issue180-secret-tool-output".to_owned(),
                    },
                )],
                duration_ms: 1,
                exit_code: Some(0),
                artifacts: Vec::new(),
                truncation: None,
                managed_output: None,
            }),
        };
        let value = serde_json::to_value(ExecutionSummary::of_tool(snapshot)).expect("serializes");
        assert_eq!(
            value["execution"],
            serde_json::json!({"kind": "tool", "id": "exec_7"}),
            "the entry carries an explicit typed handle, never a bare id"
        );
        assert_eq!(value["state"], "succeeded");
        assert_eq!(value["tool_name"], "bash");
        let mut fields = value
            .as_object()
            .expect("object")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        fields.sort();
        assert_eq!(fields, vec!["execution", "state", "tool_name"]);
        let serialized = serde_json::to_string(&value).expect("string");
        assert!(
            !serialized.contains("issue180-secret-tool-output")
                && !serialized.contains("issue180-secret-progress"),
            "a listing is never a second output channel: {serialized}"
        );
    }

    /// A subagent summary carries lifecycle/identity facts only: never the
    /// diagnostic detail, never the answer, and never the Issue #178
    /// observation plane, which `execution(status)` also withholds.
    #[test]
    fn a_subagent_summary_excludes_the_answer_and_the_observation_plane() {
        let mut snapshot = subagent_snapshot();
        snapshot.observation = crate::runtime::subagent::SubagentObservation {
            revision: 9,
            activity: crate::runtime::subagent::SubagentActivity::Tool {
                tool_call_id: crate::runtime::identity::ToolCallId::new("call-9"),
                tool_id: crate::runtime::identity::ToolId::new("tool-grep"),
                progress: None,
            },
            last_activity_at: Some(chrono::Utc::now()),
            counters: crate::runtime::subagent::SubagentActivityCounters {
                model_requests: 3,
                model_retries: 1,
                tool_executions: 2,
            },
        };
        let value =
            serde_json::to_value(ExecutionSummary::of_subagent(snapshot)).expect("serializes");
        assert_eq!(
            value["execution"],
            serde_json::json!({"kind": "subagent", "id": "conversation-1-subagent-2"}),
            "the entry carries an explicit typed handle, never a bare id"
        );
        assert_eq!(value["state"], "succeeded");
        assert_eq!(value["agent"], "explore");
        assert_eq!(value["publication_abandoned"], false);
        let mut fields = value
            .as_object()
            .expect("object")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        fields.sort();
        assert_eq!(
            fields,
            vec![
                "agent",
                "execution",
                "publication_abandoned",
                "started_at",
                "state"
            ],
            "the summary vocabulary is closed"
        );
        let serialized = serde_json::to_string(&value).expect("string");
        assert!(
            !serialized.contains("issue162-secret-child-answer"),
            "the child answer never appears in a listing: {serialized}"
        );
        for withheld in [
            "observation",
            "activity",
            "last_activity_at",
            "counters",
            "profile",
        ] {
            assert!(
                !serialized.contains(withheld),
                "the observation plane is not model-facing: {withheld} in {serialized}"
            );
        }
    }

    // -----------------------------------------------------------------
    // Ordering, bounding, and truncation (Issue #180)
    // -----------------------------------------------------------------

    /// The merged order is strict alternation starting with the tool
    /// domain, and each domain contributes in the order its own registry
    /// produced.
    #[test]
    fn the_merged_order_alternates_between_the_two_domains() {
        let listing = super::merge_bounded(tool_listing(&["t1", "t2", "t3"]), child_listing(3));
        assert_eq!(
            ids(&listing),
            vec!["t1", "c1", "t2", "c2", "t3", "c3"],
            "tool, subagent, tool, subagent, ..."
        );
        assert_eq!(listing.returned, 6);
        assert_eq!(listing.matched, 6);
        assert!(!listing.truncated);
        assert_eq!(listing.limit, MAX_LISTED_EXECUTIONS);
    }

    /// When one domain runs out, the remainder of the other follows in its
    /// own order rather than being reordered or dropped.
    #[test]
    fn an_exhausted_domain_leaves_the_other_in_its_own_order() {
        let listing =
            super::merge_bounded(tool_listing(&["t1", "t2", "t3", "t4"]), child_listing(1));
        assert_eq!(ids(&listing), vec!["t1", "c1", "t2", "t3", "t4"]);

        let listing = super::merge_bounded(tool_listing(&["t1"]), child_listing(3));
        assert_eq!(ids(&listing), vec!["t1", "c1", "c2", "c3"]);

        let listing = super::merge_bounded(tool_listing(&[]), child_listing(2));
        assert_eq!(ids(&listing), vec!["c1", "c2"]);

        let listing = super::merge_bounded(tool_listing(&["t1", "t2"]), child_listing(0));
        assert_eq!(ids(&listing), vec!["t1", "t2"]);
    }

    /// The bound is one global number: it holds however the matching
    /// executions are distributed across the two domains, and neither
    /// domain gets an independent quota.
    #[test]
    fn the_response_bound_is_global_and_never_starves_a_domain() {
        let many = (1..=MAX_LISTED_EXECUTIONS + 20)
            .map(|ordinal| format!("t{ordinal}"))
            .collect::<Vec<_>>();
        let names = many.iter().map(String::as_str).collect::<Vec<_>>();

        // One domain alone still stops exactly at the bound.
        let listing = super::merge_bounded(tool_listing(&names), child_listing(0));
        assert_eq!(listing.returned, MAX_LISTED_EXECUTIONS);
        assert_eq!(listing.matched, MAX_LISTED_EXECUTIONS + 20);
        assert!(listing.truncated);

        // With both domains overflowing, the same global bound holds and
        // alternation guarantees the smaller domain is still represented.
        let listing = super::merge_bounded(tool_listing(&names), child_listing(10));
        assert_eq!(listing.returned, MAX_LISTED_EXECUTIONS);
        assert_eq!(listing.matched, MAX_LISTED_EXECUTIONS + 30);
        assert!(listing.truncated);
        let subagents = listing
            .executions
            .iter()
            .filter(|entry| matches!(entry, ExecutionSummary::Subagent { .. }))
            .count();
        assert_eq!(
            subagents, 10,
            "alternation keeps the subagent domain visible under the bound"
        );
    }

    /// Truncation is a deterministic prefix of one deterministic order:
    /// identical listings produce identical entries and identical metadata,
    /// every time.
    #[test]
    fn truncation_is_deterministic_and_explicitly_reported() {
        let many = (1..=MAX_LISTED_EXECUTIONS + 5)
            .map(|ordinal| format!("t{ordinal}"))
            .collect::<Vec<_>>();
        let names = many.iter().map(String::as_str).collect::<Vec<_>>();
        let first = super::merge_bounded(tool_listing(&names), child_listing(7));
        let second = super::merge_bounded(tool_listing(&names), child_listing(7));
        assert_eq!(
            ids(&first),
            ids(&second),
            "the same inputs keep the same records in the same order"
        );
        assert_eq!(
            (first.returned, first.matched, first.truncated, first.limit),
            (
                second.returned,
                second.matched,
                second.truncated,
                second.limit
            ),
            "and report the same metadata"
        );
        assert_eq!(first, second, "the responses are identical");
        assert_eq!(
            ids(&first)[..6],
            ["t1", "c1", "t2", "c2", "t3", "c3"],
            "the kept records are the deterministic prefix, not a sample"
        );
        assert_eq!(first.returned, MAX_LISTED_EXECUTIONS);
        assert_eq!(first.matched, MAX_LISTED_EXECUTIONS + 12);
        assert!(first.truncated);
        assert_eq!(first.limit, MAX_LISTED_EXECUTIONS);
    }

    /// The model-facing description states the ordering contract the
    /// intrinsic actually implements (Issue #180).
    ///
    /// Only each domain is newest-first; the merged sequence is
    /// deterministic alternation, and the two domains share no ordinal or
    /// clock that could make a global chronological claim true. Promising
    /// "most recent first" would therefore have been a promise the runtime
    /// cannot keep.
    #[test]
    fn the_description_promises_determinism_not_global_recency() {
        let description = super::definition().description;
        assert!(
            description.contains("deterministically ordered"),
            "{description}"
        );
        assert!(
            description.contains("newest-first within each execution kind"),
            "{description}"
        );
        assert!(
            !description.to_lowercase().contains("most-recent-first")
                && !description.to_lowercase().contains("most recent first"),
            "the merged listing is not globally most-recent-first: {description}"
        );
    }

    /// The counts are always present, so the truncation contract does not
    /// change shape with the data.
    #[test]
    fn the_truncation_metadata_is_always_present() {
        let listing = super::merge_bounded(tool_listing(&[]), child_listing(0));
        let value = serde_json::to_value(&listing).expect("serializes");
        assert_eq!(value["executions"], serde_json::json!([]));
        assert_eq!(value["returned"], 0);
        assert_eq!(value["matched"], 0);
        assert_eq!(value["truncated"], false);
        assert_eq!(value["limit"], MAX_LISTED_EXECUTIONS);
    }

    fn ids(listing: &super::ExecutionListingResponse) -> Vec<&str> {
        listing
            .executions
            .iter()
            .map(|entry| match entry {
                ExecutionSummary::Tool { execution, .. }
                | ExecutionSummary::Subagent { execution, .. } => execution.id.as_str(),
            })
            .collect()
    }

    fn tool_listing(ids: &[&str]) -> BackgroundExecutionListing {
        let snapshots = ids
            .iter()
            .map(|id| BackgroundExecutionSnapshot {
                execution_id: ToolExecutionId::new(*id),
                tool_id: crate::runtime::identity::ToolId::new("tool-bash"),
                tool_name: "bash".to_owned(),
                state: crate::tools::background::BackgroundLifecycle::Running,
                progress: None,
                result: None,
            })
            .take(MAX_LISTED_EXECUTIONS)
            .collect::<Vec<_>>();
        BackgroundExecutionListing {
            snapshots,
            matched: ids.len(),
        }
    }

    fn child_listing(count: usize) -> SubagentListing {
        let snapshots = (1..=count)
            .map(|ordinal| {
                let mut snapshot = subagent_snapshot();
                snapshot.subagent_id =
                    crate::runtime::identity::SubagentId::new(format!("c{ordinal}"));
                snapshot
            })
            .take(MAX_LISTED_EXECUTIONS)
            .collect::<Vec<_>>();
        SubagentListing {
            snapshots,
            matched: count,
        }
    }

    fn subagent_snapshot() -> SubagentSnapshot {
        use crate::runtime::identity::{AgentId, ConversationId, SubagentId, ToolCallId};
        use crate::runtime::subagent::{
            SubagentObservation, SubagentSnapshot, SubagentState, WorkspaceSnapshot,
        };
        SubagentSnapshot {
            subagent_id: SubagentId::new("conversation-1-subagent-2"),
            child_agent_id: AgentId::new("agent-child"),
            child_conversation_id: ConversationId::new("conversation-1-subagent-2"),
            tool_call_id: ToolCallId::new("call-1"),
            agent: "explore".to_owned(),
            definition_digest: "sha256:d1".to_owned(),
            workspace: WorkspaceSnapshot::shared(std::path::PathBuf::from("<shared-workspace>")),
            handoff: None,
            state: SubagentState::Succeeded,
            // The registry-internal terminal detail carries diagnostics
            // only since Issue #178; either way the projection must drop it.
            detail: Some("issue162-secret-child-answer".to_owned()),
            observation: SubagentObservation::default(),
            profile: None,
            publication_abandoned: false,
            settled: true,
            // A frozen stamp: these projections are compared for equality,
            // and a wall-clock read would make the comparison prove the
            // clock rather than the contract.
            started_at: "2026-09-02T10:00:00Z"
                .parse::<DateTime<Utc>>()
                .expect("timestamp"),
        }
    }
}
