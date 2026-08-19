//! The conversation-owned asynchronous one-shot subagent plane (Issue #60).
//!
//! A rustX v1 subagent is a **conversation-owned, asynchronous, one-shot,
//! separate-OS-process child rustX runtime**. The child reuses the real
//! rustX stack — `ConversationRuntime`, the Agent Loop, Context Assembly,
//! the Tool Plane, and the ModelAdapter — headlessly, with an exact
//! profile-frozen capability set and an isolated conversation.
//!
//! # Ownership
//!
//! ```text
//! SubagentRegistry (this module)
//!   owns: SubagentId allocation/correlation, child identity correlation,
//!         profile identity, logical lifecycle, ownership state, capacity,
//!         cancellation intent, terminal metadata, bounded result metadata
//!   never owns: parent Ledger/Surface, parent InboundSequence allocation,
//!               parent AgentExecution admission, a private result queue,
//!               the OS process handle
//!
//! subagent process driver (subagent_process)
//!   owns: spawn, the OS child handle, the control channel, signal
//!         escalation, wait/reap, physical terminal proof
//!   never owns: canonical conversation state, lifecycle terminality
//! ```
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

mod registry;

pub(crate) mod ipc;
pub(crate) mod process;

pub use process::SubagentSpawnPlan;
pub use registry::{
    PreparedSubagent, SubagentAccepted, SubagentDurabilityFailureSink, SubagentObserver,
    SubagentRegistry, SubagentRegistryConfig, SubagentSnapshot, SubagentStartError,
    SubagentStartOutcome, SubagentStartSpec, SubagentState,
};

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

/// The explicit v1 child profile set.
///
/// A profile is **deny-by-construction**: it names the exact capability
/// set the child's frozen `ToolRegistry` contains. Unknown profiles fail
/// closed before any child ownership commit, and no v1 profile contains
/// the `subagent` tool, so recursive delegation is structurally absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentProfile {
    /// The read-only shared-workspace exploration profile: the child sees
    /// exactly `Read`, `Glob`, and `Grep` against the parent workspace and
    /// nothing else.
    Explore,
}

impl SubagentProfile {
    /// Every v1 profile, in definition order.
    pub const ALL: [Self; 1] = [Self::Explore];

    /// The stable model-facing profile name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Explore => "explore",
        }
    }

    /// Parses a profile by its stable name.
    #[must_use]
    pub fn by_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|profile| profile.name() == name)
    }

    /// The child persona/instruction text of this profile, composed as the
    /// child's bootstrap system configuration (never a forged user
    /// message).
    #[must_use]
    pub fn persona(self) -> String {
        match self {
            Self::Explore => concat!(
                "You are a read-only exploration subagent of the rustX runtime. ",
                "You answer the delegated task by inspecting the shared workspace with ",
                "the Read, Glob, and Grep capabilities only. You cannot modify anything: ",
                "no write, edit, or shell capability exists in your tool set. Produce one ",
                "bounded final answer; your runtime is one-shot and terminates with it."
            )
            .to_owned(),
        }
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

/// The durable ownership fact of one subagent child (Issue #60).
///
/// The fact carries exactly the identity a restart needs — the subagent,
/// the child agent/conversation it owns, the delegating tool call, and the
/// frozen profile — never the delegated task content, the process id, or
/// any other process-local state.
pub(crate) fn ownership_event(
    conversation_id: &ConversationId,
    subagent_id: &SubagentId,
    child_agent_id: &AgentId,
    child_conversation_id: &ConversationId,
    tool_call_id: &ToolCallId,
    profile: SubagentProfile,
    timestamp: DateTime<Utc>,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: EventId::new(format!("subagent-committed-event:{subagent_id}")),
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
            profile: profile.name().to_owned(),
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

/// The recovery-generated terminal publication of one subagent child that
/// was durably owned but never settled before the process restarted
/// (Issue #60): a runtime-authored notice with the
/// [`SubagentTerminalState::Interrupted`] terminal fact.
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
    profile: &str,
    timestamp: DateTime<Utc>,
) -> (InboundDraft, RuntimeEventEnvelope) {
    terminal_publication(
        conversation_id,
        subagent_id,
        child_agent_id,
        SubagentTerminalState::Interrupted,
        vec![UserContentBlock::Text(TextBlock {
            text: format!(
                "Subagent {subagent_id} (profile {profile}) was interrupted by a runtime \
                 restart: its actual outcome is unknown and it was not restarted."
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
