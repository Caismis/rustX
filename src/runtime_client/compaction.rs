//! Bounded checkpoint presentation over immutable Ledger statistics and exact
//! local Journal correlation. No retired Surface or message hydration.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::durable::{ConversationStore, ConversationStoreError};
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
        let (statistics, manual) = store.compaction_checkpoint(&user.id, through)?;
        entry.compaction = Some(CompactionMarker {
            retired_messages: statistics.retired_messages,
            retired_tokens: statistics.retired_tokens,
            manual,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{DefaultTokenEstimator, TokenEstimator};
    use crate::conversation::{SurfaceSpan, message_id_of};
    use crate::durable::{CompactionCommitInput, SqliteConversationStore};
    use crate::message::TextBlock;
    use crate::message::types::{
        CompactionSummaryMetadata, InboundKind, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::runtime::identity::{AttemptId, ConversationId, MessageId};
    use crate::runtime::types::{TokenMeasurement, TokenMeasurementSource};
    use std::sync::atomic::Ordering;

    fn message(id: &str, summary: bool) -> UserMessageBlock {
        UserMessageBlock {
            id: MessageId::new(id),
            content: vec![UserContentBlock::Text(TextBlock {
                text: format!("canonical content {id}"),
            })],
            source: if summary {
                UserSource::Runtime
            } else {
                UserSource::Human
            },
            kind: if summary {
                InboundKind::CompactionSummary(CompactionSummaryMetadata::empty())
            } else {
                InboundKind::Message
            },
            timestamp: None,
        }
    }
    fn commit(store: &SqliteConversationStore, id: &str, manual: bool) -> (u64, u64) {
        let head = store.load_head().unwrap();
        let retired = store.load_messages(&head.active_message_ids).unwrap();
        let expected = (
            retired.len() as u64,
            DefaultTokenEstimator.estimate_conversation_input(&retired),
        );
        store
            .commit_compaction(CompactionCommitInput {
                summary: message(id, true),
                span: SurfaceSpan::new(
                    head.active_message_ids[0].clone(),
                    head.active_message_ids.last().unwrap().clone(),
                ),
                expected_revision: head.revision,
                tokens_before: TokenMeasurement {
                    input_tokens: 10000,
                    source: TokenMeasurementSource::Estimated,
                },
                estimated_tokens_after: 20,
                attempt_id: (!manual).then(|| AttemptId::new("automatic")),
                turn_id: None,
                timestamp: chrono::Utc::now(),
                occupancy: None,
            })
            .unwrap();
        expected
    }
    fn summary_page(store: &SqliteConversationStore) -> RuntimeClientTranscriptPage {
        super::super::snapshot::transcript_page_view(store.load_transcript_page(None, 1).unwrap())
            .unwrap()
    }
    fn assert_bounded(store: &SqliteConversationStore, expected: (u64, u64), manual: Option<bool>) {
        let before = store
            .checkpoint_read_counts
            .each_ref()
            .map(|count| count.load(Ordering::SeqCst));
        let mut page = summary_page(store);
        for _ in 0..3 {
            decorate(store, &mut page, store.presentation_frontier().unwrap()).unwrap();
            let marker = page.entries[0].compaction.as_ref().unwrap();
            assert_eq!((marker.retired_messages, marker.retired_tokens), expected);
            assert_eq!(marker.manual, manual);
        }
        let after = store
            .checkpoint_read_counts
            .each_ref()
            .map(|count| count.load(Ordering::SeqCst));
        assert_eq!(after, [before[0], before[1], before[2] + 3]);
    }

    #[test]
    #[allow(clippy::too_many_lines)] // One checkpoint lifecycle and bounded-cost regression at three history scales.
    fn checkpoint_reads_are_constant_in_retired_history_and_survive_restart_and_lineage() {
        for count in [2_u64, 128, 2048] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("source.sqlite");
            let id = ConversationId::generate();
            let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
            let input: Vec<_> = (0..count)
                .map(|index| MessageBlock::User(message(&format!("input-{index}"), false)))
                .collect();
            store.initialize(&input).unwrap();
            let expected = commit(&store, "first", false);
            assert_bounded(&store, expected, Some(false));
            drop(store);
            let store = SqliteConversationStore::open(id, &path).unwrap();
            assert_bounded(&store, expected, Some(false));
            let cut = store
                .read_lineage_cut(store.load_head().unwrap().revision)
                .unwrap();
            assert!(
                crate::durable::LineageSeed::replayed(
                    cut.canonical.clone(),
                    cut.surface_history.clone(),
                    std::collections::BTreeMap::new()
                )
                .is_err()
            );
            // A child may inherit just the active summary rather than its retired
            // source history. It must retain source facts without local attribution.
            let inherited_id = MessageId::new("first");
            let inherited = crate::durable::LineageSeed::replayed(
                vec![cut.canonical.last().unwrap().clone()],
                vec![crate::conversation::SurfaceOp::Append {
                    message_id: inherited_id.clone(),
                }],
                std::collections::BTreeMap::from([(
                    inherited_id.clone(),
                    cut.checkpoints[&inherited_id],
                )]),
            )
            .unwrap();
            let inherited_child =
                SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
            inherited_child.initialize_lineage(&inherited).unwrap();
            assert_bounded(&inherited_child, expected, None);
            let seed = crate::local_runtime::session::remap_seed(
                &ConversationId::generate(),
                &cut.canonical,
                &cut.surface_history,
                &cut.checkpoints,
                &cut.completed_responses,
                &cut.turns,
            )
            .unwrap();
            let child = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
            child.initialize_lineage(&seed).unwrap();
            assert_bounded(&child, expected, None);
            child
                .initialize(&child.load_bootstrap_history().unwrap())
                .unwrap();
            assert_bounded(&child, expected, None);
            store
                .append_canonical(&MessageBlock::User(message("next", false)))
                .unwrap();
            let second = commit(&store, "second", true);
            assert_eq!(second.0, 2);
            assert_bounded(&store, second, Some(true));
            assert_eq!(
                store
                    .compaction_checkpoint(&MessageId::new("first"), u64::MAX)
                    .unwrap()
                    .0
                    .retired_messages,
                count
            );
            let historical = store
                .load_transcript_page(
                    Some(
                        store
                            .message_transcript_cursor(&MessageId::new("next"))
                            .unwrap()
                            .unwrap(),
                    ),
                    1,
                )
                .unwrap();
            let mut historical = super::super::snapshot::transcript_page_view(historical).unwrap();
            decorate(
                &store,
                &mut historical,
                store.presentation_frontier().unwrap(),
            )
            .unwrap();
            let marker = historical.entries[0].compaction.as_ref().unwrap();
            assert_eq!((marker.retired_messages, marker.retired_tokens), expected);
            assert!(store.load_messages(&[message_id_of(&input[0])]).is_ok());
        }
    }

    #[test]
    fn missing_and_stale_checkpoint_facts_are_errors() {
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        store
            .initialize(&[MessageBlock::User(message("input", false))])
            .unwrap();
        let old = store.load_head().unwrap().revision;
        commit(&store, "summary", true);
        let summary = MessageId::new("summary");
        assert!(
            store
                .compaction_checkpoint(&MessageId::new("input"), u64::MAX)
                .is_err()
        );
        let mut stale = CompactionCommitInput {
            summary: message("stale", true),
            span: SurfaceSpan::new(MessageId::new("input"), MessageId::new("input")),
            expected_revision: old,
            tokens_before: TokenMeasurement {
                input_tokens: 100,
                source: TokenMeasurementSource::Estimated,
            },
            estimated_tokens_after: 10,
            attempt_id: None,
            turn_id: None,
            timestamp: chrono::Utc::now(),
            occupancy: None,
        };
        assert!(store.commit_compaction(stale.clone()).is_err());
        assert!(
            store
                .compaction_checkpoint(&MessageId::new("stale"), u64::MAX)
                .is_err()
        );
        stale.expected_revision = store.load_head().unwrap().revision;
        assert!(store.commit_compaction(stale).is_err());
        assert_eq!(store.compaction_checkpoint(&summary, 0).unwrap().1, None);
    }
}
