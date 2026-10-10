//! Checkpoint presentation over canonical Surface history, independent of any
//! browser window. The original replaced span remains readable after restart,
//! pagination and lineage copying; summary prose is never a statistics source.
use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::context::{DefaultTokenEstimator, TokenEstimator};
use crate::conversation::SurfaceRevision;
use crate::durable::presentation::{FactQuery, FactScope};
use crate::durable::{ConversationStore, ConversationStoreError};
use crate::events::types::RuntimeEvent;
use crate::message::types::MessageBlock;

use super::snapshot::{RuntimeClientTranscriptItem, RuntimeClientTranscriptPage};

/// Facts of one checkpoint, not cumulative totals over the compaction lineage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompactionMarker {
    /// Exact number of canonical history messages replaced by this checkpoint.
    pub retired_messages: u64,
    /// Native history-only estimate, excluding system prompt and tool schemas.
    pub retired_tokens: u64,
    /// Local Journal attribution; copied lineage has no local command owner.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manual: Option<bool>,
}

pub(crate) fn decorate(
    store: &dyn ConversationStore,
    page: &mut RuntimeClientTranscriptPage,
    through: u64,
) -> Result<(), ConversationStoreError> {
    let mut checkpoints = BTreeMap::new();
    for entry in &mut page.entries {
        entry.compaction = None;
        let RuntimeClientTranscriptItem::Message {
            message: MessageBlock::User(user),
        } = &entry.item
        else {
            continue;
        };
        if !user.kind.is_compaction_summary() {
            continue;
        }
        let invalid = || {
            ConversationStoreError::InvalidReference(format!(
                "compaction summary {} has no canonical replacement span",
                user.id
            ))
        };
        let (revision, span) = store.compaction_span(&user.id)?.ok_or_else(invalid)?;
        let previous = SurfaceRevision::new(revision.get().checked_sub(1).ok_or_else(invalid)?);
        let active = store.reconstruct_surface(previous)?;
        let first = active
            .iter()
            .position(|id| id == &span.start)
            .ok_or_else(invalid)?;
        let last = active
            .iter()
            .position(|id| id == &span.end)
            .filter(|last| *last >= first)
            .ok_or_else(invalid)?;
        let retired = store.load_messages(&active[first..=last])?;
        entry.compaction = Some(CompactionMarker {
            retired_messages: retired.len() as u64,
            retired_tokens: DefaultTokenEstimator.estimate_conversation_input(&retired),
            manual: None,
        });
        checkpoints.insert(user.id.clone(), entry);
    }
    if checkpoints.is_empty() {
        return Ok(());
    }
    let mut before = None;
    loop {
        let events = store.read_presentation_events(&FactQuery {
            scope: FactScope::All,
            kinds: vec!["compaction_completed"],
            before,
            after: 0,
            ascending: false,
            through,
            limit: 64,
        })?;
        for event in &events {
            if let RuntimeEvent::CompactionCompleted {
                summary_message_id, ..
            } = &event.event
                && let Some(entry) = checkpoints.remove(summary_message_id)
                && let Some(marker) = &mut entry.compaction
            {
                marker.manual = Some(event.attempt_id.is_none());
            }
        }
        if events.len() < 64 || checkpoints.is_empty() {
            return Ok(());
        }
        before = events.last().map(|event| event.sequence);
    }
}
