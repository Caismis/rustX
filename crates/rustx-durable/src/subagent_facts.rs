use chrono::{DateTime, Utc};

use rustx_types::events::types::{
    EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope, SubagentOwnershipKind,
    SubagentTerminalState,
};
use rustx_types::message::content::TextBlock;
use rustx_types::message::types::{InboundKind, UserContentBlock, UserMessageBlock, UserSource};
use rustx_types::runtime::identity::{AgentId, ConversationId, EventId, MessageId, SubagentId};
use rustx_types::runtime::subagent::{AgentActivationOrigin, SubagentName};

use crate::inbox::InboundDraft;

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
#[must_use]
pub fn subagent_ownership_event_id(subagent_id: &SubagentId) -> EventId {
    EventId::new(format!("subagent-committed-event:{subagent_id}"))
}

/// The durable ownership fact of one subagent child (Issue #60).
///
/// The fact carries exactly the identity a restart needs — the subagent,
/// the child agent/conversation it owns, the delegating tool call, and the
/// frozen `(agent, definition_digest, profile_digest)` identity — never the
/// delegated task content, the process id, or any other process-local state.
/// Its event identity is the canonical [`subagent_ownership_event_id`] of the
/// embedded `SubagentId`.
///
/// The two digests are what make the fact self-describing across a reload,
/// and they are separate identities:
///
/// ```text
/// definition_digest  the SOURCE definition the child started with, so a
///                    later generation that redefines the same agent name
///                    cannot make an already-committed child appear to have
///                    the new definition
/// profile_digest     the EFFECTIVE execution profile the child started
///                    with, so two children of one role that an authorized
///                    invocation override specialized differently stay
///                    distinguishable after a restart
/// ```
///
/// Both are committed here, at the ownership boundary, from the frozen
/// specification. Neither is ever reconstructed from current resources.
#[allow(clippy::too_many_arguments)] // one durable fact, one construction boundary
pub fn ownership_event(
    parent_agent_id: &AgentId,
    conversation_id: &ConversationId,
    subagent_id: &SubagentId,
    child_agent_id: &AgentId,
    child_conversation_id: &ConversationId,
    origin: &AgentActivationOrigin,
    agent: &SubagentName,
    definition_digest: impl AsRef<str>,
    profile_digest: impl AsRef<str>,
    ownership: SubagentOwnershipKind,
    workspace: &std::path::Path,
    timestamp: DateTime<Utc>,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        tool_occurrence: None,
        observation: None,
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: subagent_ownership_event_id(subagent_id),
        sequence: 0,
        conversation_id: conversation_id.clone(),
        attempt_id: None,
        turn_id: None,
        timestamp,
        event: RuntimeEvent::SubagentOwnershipCommitted {
            parent_agent_id: parent_agent_id.clone(),
            admitted_authority: None,
            subagent_id: subagent_id.clone(),
            child_agent_id: child_agent_id.clone(),
            child_conversation_id: child_conversation_id.clone(),
            origin: origin.clone(),
            agent: agent.as_str().to_owned(),
            definition_digest: definition_digest.as_ref().to_owned(),
            profile_digest: profile_digest.as_ref().to_owned(),
            ownership,
            workspace: workspace.to_path_buf(),
        },
    }
}

/// The one producer correlation of a subagent terminal publication.
///
/// The live settlement path and startup recovery share this exact key, so
/// an ambiguous commit observed as an error resolves as an idempotent
/// retry and the two paths can never publish twice.
#[must_use]
pub fn terminal_correlation(subagent_id: &SubagentId) -> String {
    format!("subagent-terminal:{subagent_id}")
}

/// The deterministic message identity of a subagent terminal publication.
#[must_use]
pub fn terminal_message_id(subagent_id: &SubagentId) -> MessageId {
    MessageId::new(format!("subagent-{subagent_id}-terminal"))
}

/// The deterministic message identity of the runtime-authored terminal
/// notice accompanying a successful terminal publication (Issue #192).
#[must_use]
pub fn terminal_notice_message_id(subagent_id: &SubagentId) -> MessageId {
    MessageId::new(format!("subagent-{subagent_id}-terminal-notice"))
}

/// The one producer correlation of a successful terminal's runtime-authored
/// notice. It shares the terminal publication's exactly-once durable
/// transaction, so the bounded retry rebuilds it byte-identically and an
/// ambiguous commit resolves as the idempotent correlation retry.
#[must_use]
pub fn terminal_notice_correlation(subagent_id: &SubagentId) -> String {
    format!("subagent-terminal-notice:{subagent_id}")
}

/// Runtime-owned correlation for one final report. Stable Agent identity and
/// finite activation identity are explicit and never supplied by child content.
/// The store validates this exact notice in the same transaction as the report.
#[must_use]
pub fn terminal_notice_text(agent_id: &AgentId, subagent_id: &SubagentId, agent: &str) -> String {
    format!(
        "Agent {agent_id} activation {subagent_id} (profile \"{agent}\") completed; the message that follows \
         is its final report."
    )
}

/// The runtime-authored adjacent notice of a successful normal child
/// terminal (Issue #192).
///
/// The success report itself is child-authored, so the runtime correlation
/// metadata must not be concatenated into it — that would falsely attribute
/// runtime statements to the child. The notice is a separate
/// `UserSource::Runtime` inbound item committed in the **same** durable
/// transaction as the terminal publication, ordered strictly before the
/// report: the terminal result remains the last item of the publication.
#[must_use]
pub fn terminal_notice(
    agent_id: &AgentId,
    subagent_id: &SubagentId,
    agent: &SubagentName,
    timestamp: DateTime<Utc>,
) -> InboundDraft {
    InboundDraft {
        message_id: Some(terminal_notice_message_id(subagent_id)),
        source: UserSource::Runtime,
        kind: InboundKind::Message,
        content: vec![UserContentBlock::Text(TextBlock {
            text: terminal_notice_text(agent_id, subagent_id, agent.as_str()),
        })],
        timestamp,
        correlation: Some(terminal_notice_correlation(subagent_id)),
    }
}

/// The deterministic event identity of a subagent terminal publication.
#[must_use]
pub fn terminal_event_id(subagent_id: &SubagentId) -> EventId {
    EventId::new(format!("subagent-terminal-event:{subagent_id}"))
}

/// The deterministic event identity of a Workflow-owned child terminal
/// settlement. This is a lifecycle fact only: unlike a normal subagent
/// terminal publication it has no parent message or delivery correlation.
#[must_use]
pub fn terminal_settlement_event_id(subagent_id: &SubagentId) -> EventId {
    EventId::new(format!("subagent-terminal-settled-event:{subagent_id}"))
}

#[must_use]
pub fn admission_event_id(
    activation_id: &SubagentId,
    phase: &rustx_types::events::types::AgentActivationAdmissionPhase,
) -> EventId {
    let phase = match phase {
        rustx_types::events::types::AgentActivationAdmissionPhase::Reserved => "reserved",
        rustx_types::events::types::AgentActivationAdmissionPhase::RolledBack {
            physical_settlement_proven: false,
        } => "rollback-unproven",
        rustx_types::events::types::AgentActivationAdmissionPhase::RolledBack {
            physical_settlement_proven: true,
        } => "rolled-back",
    };
    EventId::new(format!("agent-admission:{activation_id}:{phase}"))
}

#[must_use]
pub fn admission_event(
    conversation_id: &ConversationId,
    agent_id: &AgentId,
    activation_id: &SubagentId,
    origin: &AgentActivationOrigin,
    phase: rustx_types::events::types::AgentActivationAdmissionPhase,
    timestamp: DateTime<Utc>,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        tool_occurrence: None,
        observation: None,
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: admission_event_id(activation_id, &phase),
        sequence: 0,
        conversation_id: conversation_id.clone(),
        attempt_id: None,
        turn_id: None,
        timestamp,
        event: RuntimeEvent::AgentActivationAdmission {
            agent_id: agent_id.clone(),
            activation_id: activation_id.clone(),
            origin: origin.clone(),
            phase,
        },
    }
}

/// The deterministic event identity of a committed Workflow Agent value.
/// This fact is committed in the same transaction as the corresponding
/// `SubagentTerminalSettled` lifecycle fact.
#[must_use]
pub fn physical_settlement_event_id(subagent_id: &SubagentId) -> EventId {
    EventId::new(format!("subagent-physical-settlement:{subagent_id}"))
}

#[must_use]
pub fn physical_settlement_event(
    conversation_id: &ConversationId,
    subagent_id: &SubagentId,
    child_agent_id: &AgentId,
    timestamp: DateTime<Utc>,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        tool_occurrence: None,
        observation: None,
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: physical_settlement_event_id(subagent_id),
        sequence: 0,
        conversation_id: conversation_id.clone(),
        attempt_id: None,
        turn_id: None,
        timestamp,
        event: RuntimeEvent::SubagentPhysicalSettlementProven {
            subagent_id: subagent_id.clone(),
            child_agent_id: child_agent_id.clone(),
        },
    }
}

#[must_use]
pub fn workflow_output_event_id(subagent_id: &SubagentId) -> EventId {
    EventId::new(format!("workflow-agent-output-event:{subagent_id}"))
}

/// Builds the direct terminal-settlement fact used by a Workflow-owned child.
/// The result is consumed by `WorkflowRuntime` through the native registry, so
/// no parent inbound item is created and no notification-delivery phase is
/// introduced.
#[must_use]
pub fn terminal_settlement(
    conversation_id: &ConversationId,
    subagent_id: &SubagentId,
    child_agent_id: &AgentId,
    state: SubagentTerminalState,
    physical_settlement_proven: bool,
    timestamp: DateTime<Utc>,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        tool_occurrence: None,
        observation: None,
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: terminal_settlement_event_id(subagent_id),
        sequence: 0,
        conversation_id: conversation_id.clone(),
        attempt_id: None,
        turn_id: None,
        timestamp,
        event: RuntimeEvent::SubagentTerminalSettled {
            subagent_id: subagent_id.clone(),
            child_agent_id: child_agent_id.clone(),
            state,
            physical_settlement_proven,
        },
    }
}

/// Builds the durable Workflow Agent value fact. The value is execution
/// evidence for the immutable `WorkflowRun` snapshot; it is not canonical
/// parent conversation content and never instructs recovery to replay a run.
#[allow(clippy::too_many_arguments)] // one typed Workflow terminal fact
#[must_use]
pub fn workflow_output_event(
    conversation_id: &ConversationId,
    subagent_id: &SubagentId,
    workflow_id: &rustx_types::runtime::workflow::WorkflowId,
    run_id: &rustx_types::runtime::workflow::WorkflowRunId,
    node_id: &rustx_types::runtime::workflow::WorkflowNodeInstance,
    output: serde_json::Value,
    timestamp: DateTime<Utc>,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        tool_occurrence: None,
        observation: None,
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: workflow_output_event_id(subagent_id),
        sequence: 0,
        conversation_id: conversation_id.clone(),
        attempt_id: None,
        turn_id: None,
        timestamp,
        event: RuntimeEvent::WorkflowAgentOutputCommitted {
            workflow_id: workflow_id.clone(),
            run_id: run_id.clone(),
            node_id: node_id.to_owned(),
            subagent_id: subagent_id.clone(),
            output,
        },
    }
}

/// Builds the terminal publication pair: the inbound draft (exactly-once
/// correlated) and the dependent durable terminal fact, committed together
/// through the narrow `accept_subagent_terminal` transition — the one
/// durable authority for every normal `SubagentTerminalPublished` fact
/// (Issue #192).
#[allow(clippy::too_many_arguments)] // One terminal publication carries exact identity, content and physical proof.
#[must_use]
pub fn terminal_publication(
    conversation_id: &ConversationId,
    subagent_id: &SubagentId,
    child_agent_id: &AgentId,
    state: SubagentTerminalState,
    content: Vec<UserContentBlock>,
    physical_settlement_proven: bool,
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
        tool_occurrence: None,
        observation: None,
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
            physical_settlement_proven,
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
        false,
        timestamp,
    )
}
