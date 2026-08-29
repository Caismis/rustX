//! The conversation-owned asynchronous one-shot subagent plane (Issue #60).
//!
//! A rustX v1 subagent is a **conversation-owned, asynchronous, one-shot,
//! separate-OS-process child rustX runtime**. The child reuses the real
//! rustX stack — `ConversationRuntime`, the Agent Loop, Context Assembly,
//! the Tool Plane, and the ModelAdapter — headlessly, with the exact
//! capability set frozen by its named definition and an isolated
//! conversation.
//!
//! # Ownership
//!
//! ```text
//! SubagentCatalog (catalog)
//!   owns: the immutable named definitions of one runtime resource
//!         generation and their deterministic definition digests
//!   never owns: live execution state of any kind
//!
//! SubagentResolver (resolver)
//!   owns: definition + invoking RuntimeResourceSnapshot + invoking attempt
//!         model authority -> frozen ResolvedSubagentSpec
//!   never owns: the parent's active ToolRegistry, mutable runtime-current
//!               resources, live child lifecycle
//!
//! SubagentRegistry (registry)
//!   owns: SubagentId allocation/correlation, child identity correlation,
//!         committed (agent, definition_digest) identity, logical lifecycle,
//!         ownership state, capacity, cancellation intent, terminal
//!         metadata, bounded result metadata
//!   never owns: configuration/definition semantics, parent Ledger/Surface,
//!               parent InboundSequence allocation, parent AgentExecution
//!               admission, a private result queue, the OS process handle
//!
//! subagent process driver (subagent_process)
//!   owns: spawn, the OS child handle, the control channel, signal
//!         escalation, wait/reap, physical terminal proof, and the
//!         retained nested process-unit anchors of that child (Issue #145)
//!   never owns: canonical conversation state, lifecycle terminality
//! ```
//!
//! # Nested process-unit anchors (Issue #145)
//!
//! A child that runs Bash, MCP stdio, Python/uv, or Skill environment work
//! creates supervised units whose inner `setsid()` group is outside the
//! child's own process group, so killing that group cannot reach them. Each
//! such unit offers its containment anchor to this process and may not cross
//! its local `START` gate until it is acknowledged; see
//! [`anchors`] for the parent half and
//! [`crate::runtime::nested_containment`] for the generic mechanism.
//!
//! Anchor ownership follows child ownership exactly:
//!
//! ```text
//! StagedChild   direct child process + retained anchors
//!      |  exactly-once move at the ownership commit
//!      v
//! child driver task
//! ```
//!
//! and a direct child reap is not proof of physical settlement while any
//! retained anchor is unresolved.
//!
//! # Message-bus invariant
//!
//! A subagent never writes another conversation's canonical history and
//! never schedules another conversation's attempt directly. The delegated
//! task enters the child through the child's ordinary durable inbound
//! path (`UserSource::Agent(parent)`); the child's bounded result enters
//! the parent through the parent's ordinary durable inbound acceptance
//! (`UserSource::Agent(child)` on success, `UserSource::Runtime` for
//! failure/cancellation/interruption notices). Child-process IPC only
//! transports bounded envelopes and control.

pub mod catalog;
mod registry;
pub mod resolver;

pub(crate) mod anchors;

pub(crate) mod ipc;
pub(crate) mod process;

pub use catalog::{
    CHILD_UNSAFE_BUILTIN_TOOLS, MAX_SUBAGENT_DEFINITIONS, SUBAGENT_DEFINITION_DIGEST_VERSION,
    SubagentCatalog, SubagentDefinition, SubagentDefinitionDigest, SubagentDefinitionError,
    SubagentName, SubagentNameError, SubagentProjectInstructionPolicy, SubagentToolSelector,
};
pub use process::SubagentSpawnPlan;
#[cfg(test)]
pub(crate) use registry::CommitBoundaryHook;
pub use registry::{
    PreparedSubagent, SubagentAccepted, SubagentDurabilityFailureSink, SubagentObserver,
    SubagentRegistry, SubagentRegistryConfig, SubagentSnapshot, SubagentStartError,
    SubagentStartOutcome, SubagentStartSpec, SubagentState,
};
pub use resolver::{
    ResolvedSubagentSkill, ResolvedSubagentSpec, ResolvedSubagentTool, SubagentResolutionError,
    SubagentResolver,
};

use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::durable::inbox::InboundDraft;
use crate::events::types::{
    EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope, SubagentTerminalState,
};
use crate::message::content::TextBlock;
use crate::message::types::{InboundKind, UserContentBlock, UserMessageBlock, UserSource};
use crate::runtime::identity::{
    AgentId, ConversationId, EventId, MessageId, SubagentId, ToolCallId,
};

/// The attempt-scoped subagent resolution view (Issue #144).
///
/// ```text
/// AgentExecution
///   owns Arc<RuntimeResourceSnapshot Rn>
///         |
///         v
/// ToolExecutionContext::with_subagent_context(...)
///         |
///         v
/// SubagentExecutor
///         |
///         v
/// SubagentResolver(Rn, agent)
/// ```
///
/// The invoking attempt hands the executor exactly the generation it was
/// admitted with, plus the model authority frozen at that same admission
/// boundary. The registered executor therefore never reads mutable
/// runtime-current resources, so this ordering is impossible:
///
/// ```text
/// attempt admitted under R1
/// reload commits R2
/// same attempt calls subagent
/// executor reads current R2          <- generation tearing; ruled out
/// ```
///
/// The view exposes only what resolution genuinely requires. It is not a
/// runtime handle and grants no ability to observe, mutate, or reload
/// runtime state.
///
/// The view is one shared `Arc`: it is cloned into every foreground
/// invocation of the attempt, so the frozen generation and model authority
/// are shared rather than copied into each execution future.
#[derive(Clone)]
pub struct AttemptSubagentContext {
    inner: Arc<AttemptSubagentContextInner>,
}

struct AttemptSubagentContextInner {
    resources: Arc<crate::runtime::resources::RuntimeResourceSnapshot>,
    model: crate::model::session::SessionModelConfig,
    models: crate::model::invocation::ModelBindingRegistry,
}

impl core::fmt::Debug for AttemptSubagentContext {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("AttemptSubagentContext")
            .field("resource_revision", &self.inner.resources.revision())
            .field("agents", &self.inner.resources.subagents().names())
            .finish_non_exhaustive()
    }
}

impl AttemptSubagentContext {
    /// Binds one attempt's immutable generation and frozen model authority.
    ///
    /// `model` must be the invoking attempt's **frozen effective** model
    /// configuration — the configuration captured under the same admission
    /// linearization that froze `resources` — never live mutable session
    /// state and never a composition-time capture.
    #[must_use]
    pub fn new(
        resources: Arc<crate::runtime::resources::RuntimeResourceSnapshot>,
        model: crate::model::session::SessionModelConfig,
        models: crate::model::invocation::ModelBindingRegistry,
    ) -> Self {
        Self {
            inner: Arc::new(AttemptSubagentContextInner {
                resources,
                model,
                models,
            }),
        }
    }

    /// The immutable runtime resource generation the invoking attempt owns.
    #[must_use]
    pub fn resources(&self) -> &Arc<crate::runtime::resources::RuntimeResourceSnapshot> {
        &self.inner.resources
    }

    /// Resolves one named agent against exactly this attempt's generation.
    ///
    /// # Errors
    ///
    /// Returns the first typed [`SubagentResolutionError`].
    pub fn resolve(
        &self,
        agent: &SubagentName,
    ) -> Result<ResolvedSubagentSpec, SubagentResolutionError> {
        SubagentResolver::resolve(
            &self.inner.resources,
            agent,
            &self.inner.model,
            &self.inner.models,
        )
    }

    /// The bounded model-facing routing catalog of this generation.
    #[must_use]
    pub(crate) fn routing_description(&self) -> String {
        resolver::render_agent_routing(self.inner.resources.subagents())
    }
}

/// The bounded result content bounds of the child/parent result path.
pub(crate) const MAX_RESULT_CONTENT_BYTES: usize = 64 * 1024;
/// The bounded delegated-task size.
pub(crate) const MAX_TASK_BYTES: usize = 32 * 1024;
/// The bounded explicit context-package size.
pub(crate) const MAX_CONTEXT_PACKAGE_BYTES: usize = 64 * 1024;

/// Bounds model-generated or diagnostic text by UTF-8 bytes without ever
/// splitting a Unicode scalar value.
///
/// The subagent wire and durable-publication contracts are byte bounds. The
/// greatest character boundary at or below `max_bytes` is therefore the
/// deterministic truncation point.
pub(crate) fn bound_utf8(mut value: String, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value;
    }
    let mut boundary = max_bytes;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
    value
}

/// The one canonical durable event identity of a subagent ownership fact
/// (Issue #60).
///
/// A `SubagentOwnershipCommitted` fact has exactly one deterministic
/// `EventId`, derived from the very `SubagentId` embedded in its payload:
/// `subagent-committed-event:{subagent_id}`. The durable authority enforces
/// this binding at write time and revalidates it at read/terminal-validation
/// time, so a mismatched `EventId`/`SubagentId` pair can never enter
/// durable authority and a terminal can never resolve an ownership fact
/// that does not belong to the requested child.
pub(crate) fn subagent_ownership_event_id(subagent_id: &SubagentId) -> EventId {
    EventId::new(format!("subagent-committed-event:{subagent_id}"))
}

/// The durable ownership fact of one subagent child (Issue #60).
///
/// The fact carries exactly the identity a restart needs — the subagent,
/// the child agent/conversation it owns, the delegating tool call, and the
/// frozen `(agent, definition_digest)` identity — never the delegated task
/// content, the process id, or any other process-local state. Its event
/// identity is the canonical [`subagent_ownership_event_id`] of the
/// embedded `SubagentId`.
///
/// The digest is what makes the fact self-describing across a reload: a
/// later generation that redefines the same agent name cannot make an
/// already-committed child appear to have the new definition, because the
/// durable fact names the exact definition the child started with.
#[allow(clippy::too_many_arguments)] // one durable fact, one construction boundary
pub(crate) fn ownership_event(
    conversation_id: &ConversationId,
    subagent_id: &SubagentId,
    child_agent_id: &AgentId,
    child_conversation_id: &ConversationId,
    tool_call_id: &ToolCallId,
    agent: &SubagentName,
    definition_digest: &SubagentDefinitionDigest,
    timestamp: DateTime<Utc>,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: subagent_ownership_event_id(subagent_id),
        sequence: 0,
        conversation_id: conversation_id.clone(),
        attempt_id: None,
        turn_id: None,
        timestamp,
        event: RuntimeEvent::SubagentOwnershipCommitted {
            subagent_id: subagent_id.clone(),
            child_agent_id: child_agent_id.clone(),
            child_conversation_id: child_conversation_id.clone(),
            tool_call_id: tool_call_id.clone(),
            agent: agent.as_str().to_owned(),
            definition_digest: definition_digest.as_str().to_owned(),
        },
    }
}

/// The one producer correlation of a subagent terminal publication.
///
/// The live settlement path and startup recovery share this exact key, so
/// an ambiguous commit observed as an error resolves as an idempotent
/// retry and the two paths can never publish twice.
pub(crate) fn terminal_correlation(subagent_id: &SubagentId) -> String {
    format!("subagent-terminal:{subagent_id}")
}

/// The deterministic message identity of a subagent terminal publication.
pub(crate) fn terminal_message_id(subagent_id: &SubagentId) -> MessageId {
    MessageId::new(format!("subagent-{subagent_id}-terminal"))
}

/// The deterministic event identity of a subagent terminal publication.
pub(crate) fn terminal_event_id(subagent_id: &SubagentId) -> EventId {
    EventId::new(format!("subagent-terminal-event:{subagent_id}"))
}

/// Builds the terminal publication pair: the inbound draft (exactly-once
/// correlated) and the dependent durable terminal fact, committed together
/// through the narrow `accept_inbound_with_event` transition.
pub(crate) fn terminal_publication(
    conversation_id: &ConversationId,
    subagent_id: &SubagentId,
    child_agent_id: &AgentId,
    state: SubagentTerminalState,
    content: Vec<UserContentBlock>,
    timestamp: DateTime<Utc>,
) -> (InboundDraft, RuntimeEventEnvelope) {
    debug_assert!(
        matches!(state, SubagentTerminalState::Succeeded)
            == matches!(
                content_source(state, child_agent_id),
                UserSource::Agent { .. }
            ),
        "a successful terminal is authored by the child agent; every other terminal is a runtime notice"
    );
    let message = UserMessageBlock {
        id: terminal_message_id(subagent_id),
        content,
        source: content_source(state, child_agent_id),
        kind: InboundKind::Message,
        timestamp: Some(timestamp),
    };
    let event = RuntimeEventEnvelope {
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: terminal_event_id(subagent_id),
        sequence: 0,
        conversation_id: conversation_id.clone(),
        attempt_id: None,
        turn_id: None,
        timestamp,
        event: RuntimeEvent::SubagentTerminalPublished {
            subagent_id: subagent_id.clone(),
            child_agent_id: child_agent_id.clone(),
            message_id: message.id.clone(),
            state,
        },
    };
    let draft = InboundDraft {
        message_id: Some(message.id.clone()),
        source: message.source,
        kind: message.kind,
        content: message.content,
        timestamp,
        correlation: Some(terminal_correlation(subagent_id)),
    };
    (draft, event)
}

/// The provenance of one terminal publication: a successful answer is
/// authored by the child agent; every other terminal is the runtime
/// speaking about the child.
fn content_source(state: SubagentTerminalState, child_agent_id: &AgentId) -> UserSource {
    match state {
        SubagentTerminalState::Succeeded => UserSource::Agent {
            agent_id: child_agent_id.clone(),
        },
        SubagentTerminalState::Failed
        | SubagentTerminalState::Cancelled
        | SubagentTerminalState::Interrupted => UserSource::Runtime,
    }
}

/// The runtime-authored terminal publication of one subagent child whose
/// process/IPC outcome is unknown (live physical loss or restart recovery):
/// a bounded notice with the [`SubagentTerminalState::Interrupted`] fact.
///
/// The identity contract is deliberately identical to the live settlement
/// path — the same `MessageId` and the same producer correlation — so a
/// live publication and a recovery publication are mutually exclusive by
/// construction. Nothing is relaunched and no old process is reattached.
#[must_use]
pub fn recovery_terminal_publication(
    conversation_id: &ConversationId,
    subagent_id: &SubagentId,
    child_agent_id: &AgentId,
    agent: &str,
    definition_digest: &str,
    timestamp: DateTime<Utc>,
) -> (InboundDraft, RuntimeEventEnvelope) {
    terminal_publication(
        conversation_id,
        subagent_id,
        child_agent_id,
        SubagentTerminalState::Interrupted,
        vec![UserContentBlock::Text(TextBlock {
            text: format!(
                "Subagent {subagent_id} (agent {agent}, definition {definition_digest}) was \
                 interrupted by a runtime restart: its actual outcome is unknown and it was \
                 not restarted."
            ),
        })],
        timestamp,
    )
}

#[cfg(test)]
mod tests {
    use super::bound_utf8;

    #[test]
    fn utf8_bounds_are_byte_caps_at_character_boundaries() {
        let chinese = "界".repeat(32);
        let chinese_bound = bound_utf8(chinese.clone(), 65);
        assert!(chinese_bound.len() <= 65);
        assert_eq!(chinese_bound.len() % "界".len(), 0);
        assert_eq!(chinese_bound, "界".repeat(21));

        let emoji = "🙂".repeat(20);
        let emoji_bound = bound_utf8(emoji.clone(), 65);
        assert!(emoji_bound.len() <= 65);
        assert_eq!(emoji_bound.len() % "🙂".len(), 0);
        assert_eq!(emoji_bound, "🙂".repeat(16));

        assert_eq!(bound_utf8("ascii".to_owned(), 5), "ascii");
        assert_eq!(bound_utf8("ascii".to_owned(), 4), "asci");
        assert_eq!(bound_utf8("short🙂".to_owned(), 64), "short🙂");
    }
}
