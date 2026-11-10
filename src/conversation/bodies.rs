//! Native canonical body allocations shared by the Ledger and actual input.
//! This registry owns only weak leases; it cannot keep retired history alive.
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, Weak};

use crate::message::types::MessageBlock;
use crate::model::ModelInputMessage;

/// A bounded session-local registry of recent canonical allocations.
/// Values enter only after native canonical validation, never through Source
/// or Trace. Exact encoded length and digest prevent identity-only or numeric
/// equality substitution (notably positive versus negative zero).
#[derive(Debug, Default)]
pub struct CanonicalBodies {
    recent: Mutex<VecDeque<(BodyKey, Weak<ModelInputMessage>)>>,
}

impl CanonicalBodies {
    pub(crate) fn admit(&self, body: Arc<ModelInputMessage>) -> Arc<ModelInputMessage> {
        let message = body.as_canonical().expect("native canonical body");
        let key = body_key(message);
        let mut recent = self.recent.lock().expect("canonical body registry");
        for (prior_key, weak) in recent.iter().rev() {
            if *prior_key == key
                && let Some(prior) = weak.upgrade()
            {
                return prior;
            }
        }
        recent.push_back((key, Arc::downgrade(&body)));
        if recent.len() > 64 {
            recent.pop_front();
        }
        body
    }

    /// Share an immutable canonical value already validated by its native owner.
    #[must_use]
    pub(crate) fn share(&self, message: MessageBlock) -> Arc<ModelInputMessage> {
        let key = body_key(&message);
        let mut recent = self.recent.lock().expect("canonical body registry");
        for (prior_key, weak) in recent.iter().rev() {
            if *prior_key == key
                && let Some(body) = weak.upgrade()
            {
                return body;
            }
        }
        let body = Arc::new(ModelInputMessage::Canonical(message));
        recent.push_back((key, Arc::downgrade(&body)));
        if recent.len() > 64 {
            recent.pop_front();
        }
        body
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BodyKey {
    bytes: usize,
    digest: [u8; 32],
}

fn body_key(message: &MessageBlock) -> BodyKey {
    let (bytes, digest) = crate::model::projected::canonical_body_identity(message);
    BodyKey {
        bytes,
        digest: digest.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::ConversationState;
    use crate::message::types::{InboundKind, UserMessageBlock, UserSource};
    use crate::message::{TextBlock, UserContentBlock};
    use crate::model::SelectedInputManifest;
    use crate::runtime::identity::{ConversationId, MessageId};

    fn user(index: usize, text: &str) -> MessageBlock {
        MessageBlock::User(UserMessageBlock {
            id: MessageId::new(format!("shared-{index}")),
            content: vec![UserContentBlock::Text(TextBlock { text: text.into() })],
            kind: InboundKind::Message,
            source: UserSource::Human,
            timestamp: None,
        })
    }

    #[test]
    fn canonical_ledger_surface_and_actual_lease_the_same_body_at_8_64_512_4096() {
        let scope = ConversationId::generate();
        let bodies = Arc::new(CanonicalBodies::default());
        let mut state = ConversationState::new();
        state.bind_bodies(Arc::clone(&bodies));
        let mut input = SelectedInputManifest::default();
        for h in 1..=4096 {
            let message = user(h, "canonical body");
            let id = state.commit(message.clone()).unwrap();
            input = input.append_shared(&scope, bodies.share(message));
            if [8, 64, 512, 4096].contains(&h) {
                assert_eq!(bodies.recent.lock().unwrap().len(), h.min(64));
                let selected = input.messages();
                for i in 0..h {
                    let actual = selected.get(i).unwrap().as_canonical().unwrap();
                    assert!(std::ptr::eq(
                        actual,
                        state.ledger().get(actual.id()).unwrap()
                    ));
                    assert!(std::ptr::eq(
                        actual,
                        state
                            .freeze_active_surface()
                            .messages
                            .get(actual.id())
                            .unwrap()
                    ));
                }
                let original = selected.shared(h - 1);
                state.bind_bodies(Arc::clone(&bodies));
                assert!(std::ptr::eq(
                    original.as_canonical().unwrap(),
                    state.ledger().get(&id).unwrap()
                ));
            }
        }
    }

    #[test]
    fn registry_preserves_the_exact_sign_of_zero() {
        use crate::message::types::{AssistantContentBlock, AssistantMessageBlock};
        use crate::runtime::identity::{ToolCallId, ToolId};
        use crate::tools::types::ToolCall;
        let message = |number: &str| {
            MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new("numeric-owner"),
                content: vec![AssistantContentBlock::ToolCall(ToolCall {
                    id: ToolCallId::new("call"),
                    tool_id: ToolId::new("tool"),
                    name: "tool".into(),
                    arguments: serde_json::from_str(number).unwrap(),
                })],
            })
        };
        let negative = message("-0.0");
        let positive = message("0.0");
        let bodies = CanonicalBodies::default();
        let a = bodies.share(negative.clone());
        let b = bodies.share(positive.clone());
        assert!(!Arc::ptr_eq(&a, &b));
        assert_eq!(
            serde_json::to_vec(a.as_canonical().unwrap()).unwrap(),
            serde_json::to_vec(&negative).unwrap()
        );
        assert_eq!(
            serde_json::to_vec(b.as_canonical().unwrap()).unwrap(),
            serde_json::to_vec(&positive).unwrap()
        );
    }

    #[test]
    fn registry_does_not_keep_retired_bodies_alive_or_substitute_changed_content() {
        let bodies = CanonicalBodies::default();
        let original = bodies.share(user(1, "exact old cut"));
        let same = bodies.share(user(1, "exact old cut"));
        assert!(Arc::ptr_eq(&original, &same));
        let changed = bodies.share(user(1, "changed content"));
        assert!(!Arc::ptr_eq(&original, &changed));
        assert_eq!(original.as_canonical(), Some(&user(1, "exact old cut")));
        let weak = Arc::downgrade(&original);
        drop(original);
        drop(same);
        assert!(weak.upgrade().is_none());
        for i in 2..=4096 {
            let _ = bodies.share(user(i, "transient"));
        }
        assert_eq!(bodies.recent.lock().unwrap().len(), 64);
        assert!(
            bodies
                .recent
                .lock()
                .unwrap()
                .iter()
                .all(|(_, body)| body.upgrade().is_none())
        );
    }
}
