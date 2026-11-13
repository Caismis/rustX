//! Native task provenance supplied to permission review, separate from model
//! arguments and from the model-authored elevation reason.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum PermissionTaskEvidence {
    /// The canonical input retains its exact Human/Agent provenance;
    /// receiving an Agent message never promotes it into Human authorization.
    CanonicalInput {
        message: crate::message::UserMessageBlock,
    },
    /// The admitted scheduler Claim is the execution authority of this task.
    ScheduledClaim { claim: serde_json::Value },
}

impl PermissionTaskEvidence {
    pub(crate) fn message_id(&self) -> Option<&crate::runtime::identity::MessageId> {
        match self {
            Self::CanonicalInput { message } => Some(&message.id),
            Self::ScheduledClaim { .. } => None,
        }
    }

    pub(crate) fn canonical(message: &crate::message::MessageBlock) -> Option<Self> {
        match message {
            crate::message::MessageBlock::User(message)
                if message.kind == crate::message::InboundKind::Message
                    && matches!(
                        message.source,
                        crate::message::UserSource::Human
                            | crate::message::UserSource::Agent { .. }
                    ) =>
            {
                Some(Self::CanonicalInput {
                    message: message.clone(),
                })
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{InboundKind, MessageBlock, UserMessageBlock, UserSource};
    use crate::runtime::identity::{AgentId, MessageId};

    #[test]
    fn task_evidence_requires_ordinary_human_or_delegated_input() {
        for (source, authorized) in [
            (UserSource::Human, true),
            (
                UserSource::Agent {
                    agent_id: AgentId::new("parent"),
                },
                true,
            ),
            (UserSource::Runtime, false),
            (UserSource::Fleet, false),
            (UserSource::ExternalSystem, false),
        ] {
            let mut message = UserMessageBlock {
                id: MessageId::new("task-source"),
                source,
                kind: InboundKind::Message,
                timestamp: None,
                content: Vec::new(),
            };
            let evidence = PermissionTaskEvidence::canonical(&MessageBlock::User(message.clone()));
            assert_eq!(evidence.is_some(), authorized, "{:?}", message.source);
            if let Some(PermissionTaskEvidence::CanonicalInput { message: actual }) = evidence {
                assert_eq!(actual, message);
            }
            message.kind = InboundKind::QueuedMessage;
            assert!(PermissionTaskEvidence::canonical(&MessageBlock::User(message)).is_none());
        }
    }
}
