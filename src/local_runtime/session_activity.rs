//! Session recency follows committed human messages, never focus or metadata edits.
//! The publisher observes every node of a Session while retaining only a weak catalog.
use std::sync::Weak;

use chrono::{DateTime, Utc};

use crate::message::types::{InboundKind, MessageBlock, UserMessageBlock, UserSource};
use crate::runtime::conversation_runtime::ConversationRuntime;
use crate::runtime::observation::ConversationObservation;

use super::session::{SessionCatalog, SessionId};

pub(crate) fn human_message_time(user: &UserMessageBlock) -> Option<DateTime<Utc>> {
    (user.kind == InboundKind::Message && user.source == UserSource::Human)
        .then_some(user.timestamp)
        .flatten()
}

fn collect_activity(observation: &ConversationObservation, latest: &mut Option<DateTime<Utc>>) {
    match observation {
        ConversationObservation::Committed {
            block: MessageBlock::User(user),
            ..
        } => {
            if let Some(time) = human_message_time(user) {
                *latest = Some(latest.map_or(time, |previous| previous.max(time)));
            }
        }
        ConversationObservation::JournalBatch { observations, .. } => {
            for observation in observations {
                collect_activity(observation, latest);
            }
        }
        ConversationObservation::Published { observation, .. } => {
            collect_activity(observation, latest);
        }
        _ => {}
    }
}

pub(crate) fn arm_activity(
    catalog: Weak<tokio::sync::Mutex<SessionCatalog>>,
    session_id: SessionId,
    runtime: &ConversationRuntime,
) {
    let observations = match runtime.subscribe_observations() {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(%session_id, %error, "Session activity subscription failed");
            return;
        }
    };
    tokio::spawn(async move {
        loop {
            observations.wait().await;
            let mut latest = None;
            for observation in observations.drain() {
                collect_activity(&observation, &mut latest);
            }
            if let Some(time) = latest {
                let Some(catalog) = catalog.upgrade() else {
                    return;
                };
                if let Err(error) = catalog.lock().await.publish_activity(&session_id, time) {
                    tracing::warn!(%session_id, %error, "Session activity publication failed");
                }
            }
            if observations.is_closed() {
                return;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::identity::{AgentId, MessageId};

    #[test]
    fn only_committed_human_messages_advance_activity() {
        let time = DateTime::parse_from_rfc3339("2026-10-07T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut message = UserMessageBlock {
            id: MessageId::new("human"),
            content: vec![],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: Some(time),
        };
        let mut latest = None;
        let observe = |message: UserMessageBlock, latest: &mut Option<DateTime<Utc>>| {
            collect_activity(
                &ConversationObservation::Committed {
                    attempt_id: None,
                    block: MessageBlock::User(message),
                    transcript_cursor: None,
                },
                latest,
            );
        };
        observe(message.clone(), &mut latest);
        assert_eq!(latest, Some(time));
        message.timestamp = Some(time + chrono::Duration::hours(1));
        for source in [
            UserSource::Agent {
                agent_id: AgentId::new("agent"),
            },
            UserSource::Fleet,
            UserSource::ExternalSystem,
            UserSource::Runtime,
        ] {
            message.source = source;
            observe(message.clone(), &mut latest);
            assert_eq!(latest, Some(time));
        }
        message.source = UserSource::Human;
        message.kind = InboundKind::CompactionSummary(
            crate::message::types::CompactionSummaryMetadata::new(vec![], vec![]).unwrap(),
        );
        observe(message.clone(), &mut latest);
        assert_eq!(latest, Some(time));
        message.kind = InboundKind::Message;
        message.timestamp = None;
        observe(message.clone(), &mut latest);
        assert_eq!(latest, Some(time));
        message.timestamp = Some(time - chrono::Duration::hours(1));
        observe(message.clone(), &mut latest);
        assert_eq!(latest, Some(time));
        message.timestamp = Some(time + chrono::Duration::hours(2));
        observe(message, &mut latest);
        assert_eq!(latest, Some(time + chrono::Duration::hours(2)));
    }
}
