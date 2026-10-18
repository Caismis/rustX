//! Immutable canonical Surface selection. Bodies share the Ledger's allocations;
//! derived model input is never admitted here. Aggregate status metadata belongs
//! to this same selection and is updated only when its canonical append commits.
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer, ser::SerializeSeq};

use crate::message::types::{AgentStatusModuleId, MessageBlock};
use crate::runtime::identity::MessageId;

/// A persistent ordered selection of immutable canonical bodies.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CanonicalSelection {
    records: imbl::Vector<Arc<MessageBlock>>,
    positions: imbl::HashMap<MessageId, (usize, usize)>,
    non_status: usize,
    latest_status: [Option<(MessageId, DateTime<Utc>)>; 3],
}

impl CanonicalSelection {
    /// Number of selected canonical messages.
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }
    /// Whether the selection is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
    /// Iterate borrowed immutable canonical bodies without hydration or copying.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &MessageBlock> + ExactSizeIterator {
        self.records.iter().map(Arc::as_ref)
    }
    /// Explicitly materialize ordered identities at an external read boundary.
    #[must_use]
    pub fn ids(&self) -> Vec<MessageId> {
        self.iter().map(|m| m.id().clone()).collect()
    }
    /// Position in this exact canonical selection.
    #[must_use]
    pub fn position(&self, id: &MessageId) -> Option<usize> {
        self.positions.get(id).map(|p| p.0)
    }
    /// Resolve a body from this selection, never from a different historical cut.
    #[must_use]
    pub fn get(&self, id: &MessageId) -> Option<&MessageBlock> {
        self.position(id).map(|p| self.records[p].as_ref())
    }
    /// Most recent typed status generation for a module.
    #[must_use]
    pub fn latest_status(
        &self,
        module: AgentStatusModuleId,
    ) -> Option<&(MessageId, DateTime<Utc>)> {
        self.latest_status[module_index(module)].as_ref()
    }
    /// Number of ordinary messages after an identity in this exact selection.
    #[must_use]
    pub fn non_status_since(&self, id: &MessageId) -> Option<usize> {
        self.positions.get(id).map(|p| self.non_status - p.1)
    }
    pub(crate) fn append(&mut self, message: Arc<MessageBlock>) {
        let id = message.id().clone();
        assert!(
            !self.positions.contains_key(&id),
            "duplicate canonical selection identity"
        );
        if let Some(metadata) = message.agent_status_metadata() {
            for module in metadata.modules() {
                self.latest_status[module_index(*module)] =
                    Some((id.clone(), metadata.generated_at()));
            }
        } else {
            self.non_status += 1;
        }
        self.positions
            .insert(id, (self.records.len(), self.non_status));
        self.records.push_back(message);
    }
}

const fn module_index(module: AgentStatusModuleId) -> usize {
    match module {
        AgentStatusModuleId::Time => 0,
        AgentStatusModuleId::Background => 1,
        AgentStatusModuleId::Todo => 2,
    }
}

impl Serialize for CanonicalSelection {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.len()))?;
        for message in self.iter() {
            seq.serialize_element(message)?;
        }
        seq.end()
    }
}
impl<'de> Deserialize<'de> for CanonicalSelection {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let messages = Vec::<MessageBlock>::deserialize(deserializer)?;
        let mut selection = Self::default();
        for message in messages {
            if selection.position(message.id()).is_some() {
                return Err(serde::de::Error::custom(
                    "duplicate canonical selection identity",
                ));
            }
            selection.append(Arc::new(message));
        }
        Ok(selection)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::{ConversationState, SurfaceRevision};
    use crate::message::types::{InboundKind, UserMessageBlock, UserSource};

    fn user(id: &str) -> MessageBlock {
        MessageBlock::User(UserMessageBlock {
            id: MessageId::new(id),
            content: Vec::new(),
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        })
    }

    #[test]
    fn canonical_freezes_share_old_bodies_and_preserve_the_exact_selection_at_100_300_1000() {
        let mut state = ConversationState::new();
        for count in 1..=1000 {
            state.commit(user(&format!("canonical-{count}"))).unwrap();
            if ![100, 300, 1000].contains(&count) {
                continue;
            }
            state.ledger_access().reset();
            let before = state.freeze_active_surface();
            let same = state.freeze_active_surface();
            assert!(before.messages.records.ptr_eq(&same.messages.records));
            assert_eq!(state.ledger_access().keyed_reads(), 0);
            assert_eq!(state.ledger_access().enumerations(), 0);
            let first = MessageId::new("canonical-1");
            assert!(std::ptr::eq(
                before.messages.get(&first).unwrap(),
                state.ledger().get(&first).unwrap()
            ));
            let mut appended = before.messages.clone();
            appended.append(Arc::new(user("next-independent-snapshot")));
            assert_eq!(before.messages.len(), count);
            assert_eq!(appended.len(), count + 1);
            assert!(std::ptr::eq(
                before.messages.get(&first).unwrap(),
                appended.get(&first).unwrap()
            ));
            assert_eq!(before.messages.non_status_since(&first), Some(count - 1));
            assert_eq!(appended.non_status_since(&first), Some(count));
            let bytes = serde_json::to_vec(&before.messages).unwrap();
            let reopened: CanonicalSelection = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(reopened, before.messages);
        }
    }

    #[test]
    fn canonical_durable_selection_does_not_substitute_ledger_order_or_model_projection() {
        let state = ConversationState::from_durable_head(
            [user("a"), user("b"), user("c")],
            vec![MessageId::new("c"), MessageId::new("a")],
            SurfaceRevision::new(20),
            3,
        )
        .unwrap();
        let selected = state.freeze_active_surface();
        assert_eq!(
            selected.messages.ids(),
            [MessageId::new("c"), MessageId::new("a")]
        );
        assert!(selected.messages.get(&MessageId::new("b")).is_none());
        assert!(state.ledger().get(&MessageId::new("b")).is_some());
        assert_eq!(
            selected.messages.non_status_since(&MessageId::new("c")),
            Some(1)
        );
        assert_eq!(selected.compaction_generation, 3);
    }
}
