//! Durable publication-draft facts for detached background executions.
//!
//! Constructors for the journal facts a restart recovery or a live terminal
//! transition commits — pure vocabulary assembly, no store access.
use crate::inbox::InboundDraft;
use rustx_types::{
    events::types::{
        BackgroundTerminalState, EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope,
    },
    message::types::UserMessageBlock,
    runtime::identity::{ConversationId, EventId, MessageId, ToolExecutionId},
    tools::background::BackgroundLifecycle,
};

pub fn recovery_terminal_publication(
    conversation_id: &ConversationId,
    execution_id: &ToolExecutionId,
    tool_name: &str,
    timestamp: chrono::DateTime<chrono::Utc>,
) -> (crate::inbox::InboundDraft, RuntimeEventEnvelope) {
    use rustx_types::{
        events::types::BackgroundTerminalState,
        message::{
            content::TextBlock,
            types::{InboundKind, UserContentBlock, UserMessageBlock, UserSource},
        },
        runtime::identity::EventId,
    };

    let notification = UserMessageBlock {
        id: MessageId::new(format!("background-{}-terminal", execution_id.as_str())),
        content: vec![UserContentBlock::Text(TextBlock {
            text: format!(
                "Background execution {} ({tool_name}) was interrupted by a runtime restart: \
                 its actual outcome is unknown and it was not restarted.",
                execution_id.as_str()
            ),
        })],
        source: UserSource::Runtime,
        kind: InboundKind::Message,
        timestamp: Some(timestamp),
    };
    let event = background_publication_event(
        conversation_id,
        &EventId::new(format!("background-terminal-event:{execution_id}")),
        &notification,
        execution_id,
        BackgroundTerminalState::OutcomeUnknown,
    );
    let correlation = format!("background-terminal:{}", execution_id.as_str());
    (inbound_draft(notification, correlation), event)
}

pub fn background_publication_event(
    conversation_id: &ConversationId,
    event_id: &EventId,
    notification: &rustx_types::message::types::UserMessageBlock,
    execution_id: &ToolExecutionId,
    state: rustx_types::events::types::BackgroundTerminalState,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        tool_occurrence: None,
        observation: None,
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: event_id.clone(),
        sequence: 0,
        conversation_id: conversation_id.clone(),
        attempt_id: None,
        turn_id: None,
        timestamp: notification
            .timestamp
            .expect("background terminal notifications carry a timestamp"),
        event: RuntimeEvent::BackgroundTerminalPublished {
            execution_id: execution_id.clone(),
            message_id: notification.id.clone(),
            state,
        },
    }
}

pub fn inbound_draft(notification: UserMessageBlock, correlation: String) -> InboundDraft {
    let timestamp = notification
        .timestamp
        .expect("background terminal notifications carry a timestamp");
    InboundDraft {
        message_id: Some(notification.id.clone()),
        source: notification.source,
        kind: notification.kind,
        content: notification.content,
        timestamp,
        correlation: Some(correlation),
    }
}

pub fn background_terminal_event(
    conversation_id: &ConversationId,
    event_id: &EventId,
    notification: &UserMessageBlock,
    execution_id: &ToolExecutionId,
    state: BackgroundLifecycle,
) -> RuntimeEventEnvelope {
    background_publication_event(
        conversation_id,
        event_id,
        notification,
        execution_id,
        match state {
            BackgroundLifecycle::Succeeded => BackgroundTerminalState::Succeeded,
            BackgroundLifecycle::Failed => BackgroundTerminalState::Failed,
            BackgroundLifecycle::Denied => BackgroundTerminalState::Denied,
            BackgroundLifecycle::Cancelled => BackgroundTerminalState::Cancelled,
            BackgroundLifecycle::TimedOut => BackgroundTerminalState::TimedOut,
            BackgroundLifecycle::OutcomeUnknown => BackgroundTerminalState::OutcomeUnknown,
            BackgroundLifecycle::Starting
            | BackgroundLifecycle::Running
            | BackgroundLifecycle::Cancelling
            | BackgroundLifecycle::PublishingTerminal => {
                unreachable!("only terminal background states are published")
            }
        },
    )
}
