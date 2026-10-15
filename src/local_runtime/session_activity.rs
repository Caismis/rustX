//! Session recency follows committed human messages, never focus or metadata edits.
//! The publisher observes every node of a Session while retaining only a weak catalog.
use std::sync::Weak;

use chrono::{DateTime, Utc};

#[cfg(test)]
use crate::message::types::MessageBlock;
use crate::message::types::{InboundKind, UserMessageBlock, UserSource};
use crate::runtime::conversation_runtime::ConversationRuntime;
#[cfg(test)]
use crate::runtime::observation::ConversationObservation;

use super::session::{SessionCatalog, SessionId};

pub(crate) fn human_message_time(user: &UserMessageBlock) -> Option<DateTime<Utc>> {
    (user.kind == InboundKind::Message && user.source == UserSource::Human)
        .then_some(user.timestamp)
        .flatten()
}

#[cfg(test)]
fn collect_activity(observation: &ConversationObservation, latest: &mut Option<DateTime<Utc>>) {
    for fact in observation.facts() {
        let time = match fact {
            ConversationObservation::CanonicalCommitted { receipt, .. } => {
                receipt.observation.human_activity_at
            }
            #[cfg(test)]
            ConversationObservation::Committed {
                block: MessageBlock::User(user),
                ..
            } => human_message_time(user),
            _ => None,
        };
        if let Some(time) = time {
            *latest = Some(latest.map_or(time, |previous| previous.max(time)));
        }
    }
}

pub(crate) fn arm_activity(
    catalog: Weak<tokio::sync::Mutex<SessionCatalog>>,
    session_id: SessionId,
    runtime: &ConversationRuntime,
) {
    let mut observations = match runtime.session_observation_metadata() {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(%session_id, %error, "Session activity subscription failed");
            return;
        }
    };
    tokio::spawn(async move {
        let mut published = None;
        loop {
            // A watch retains exactly one native aggregate while Catalog I/O
            // is blocked; no canonical body or Journal callback is queued.
            let metadata = observations.borrow_and_update().clone();
            if let Some(time) = metadata.human_activity_at
                && published.is_none_or(|previous| previous < time)
            {
                let Some(catalog) = catalog.upgrade() else {
                    return;
                };
                if let Err(error) = catalog.lock().await.publish_activity(&session_id, time) {
                    tracing::warn!(%session_id, %error, "Session activity publication failed");
                }
                published = Some(time);
            }
            if metadata.closed || observations.changed().await.is_err() {
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
