use super::*;
use crate::message::types::ContentBlockIndex;
use crate::runtime::identity::{ConversationId, MessageId, ToolCallId, ToolId};
use crate::runtime_client::content::{
    CanonicalContentComponent, CanonicalContentLocator, ContentRepresentation, ContentSource,
};
use crate::runtime_client::snapshot::InFlightBlock;

fn original(capture: &mut Capture, block: &InFlightBlock) -> Option<InFlightBlock> {
    bounded_clone(block, CHECKPOINT_ITEM_BYTES).filter(|value| capture.reserve(value))
}

fn variants(text: &str) -> Vec<InFlightBlock> {
    vec![
        InFlightBlock::Text {
            block_index: ContentBlockIndex::new(u32::MAX),
            text: text.into(),
        },
        InFlightBlock::Reasoning {
            block_index: ContentBlockIndex::new(1),
            text: text.into(),
        },
        InFlightBlock::Refusal {
            block_index: ContentBlockIndex::new(2),
            text: text.into(),
        },
        InFlightBlock::ToolCall {
            block_index: ContentBlockIndex::new(3),
            call_id: Some(ToolCallId::new("call\"水")),
            tool_id: Some(ToolId::new("tool\n")),
            metadata_ref: Some(ContentRef {
                conversation_id: ConversationId::new("conv_601cccec-75c7-703d-af18-cdf2cb42a8d1"),
                source: ContentSource::CanonicalMessageBlock {
                    component: CanonicalContentComponent::BlockMetadata,
                    locator: CanonicalContentLocator::ByMessageId {
                        message_id: MessageId::new("message"),
                    },
                    block_index: ContentBlockIndex::new(3),
                },
                representation: ContentRepresentation::CanonicalJsonV1,
                version: u16::MAX,
                content_revision: u64::MAX,
                readable_end: u64::MAX,
            }),
            name: "tool 水".into(),
            // These are lexical provider fragments, including invalid JSON.
            // They remain strings rather than being parsed as argument JSON.
            arguments: text.into(),
        },
    ]
}

#[test]
fn live_capture_matches_original_bytes_and_budgets_at_both_admission_boundaries() {
    for text in [
        String::new(),
        "  {\"wide\":1e+003,\"unfinished\": ".into(),
        "水\"\\\n\0".repeat(120),
        "x".repeat(CHECKPOINT_ITEM_BYTES - 64),
        "水".repeat(CHECKPOINT_ITEM_BYTES),
    ] {
        for block in variants(&text) {
            let encoded = serde_json::to_vec(&block).unwrap().len();
            let required =
                crate::runtime_client::preflight::required_budget(&block, CHECKPOINT_TARGET_BYTES)
                    .unwrap();
            for remaining in [
                0,
                1,
                encoded.saturating_sub(1),
                encoded,
                required.saturating_sub(1),
                required,
                required + 1,
                CHECKPOINT_ITEM_BYTES,
                CHECKPOINT_TARGET_BYTES / 8,
            ] {
                let mut expected = Capture::new(7);
                expected.remaining = remaining.min(CHECKPOINT_TARGET_BYTES / 8);
                let mut actual = Capture::new(7);
                actual.remaining = expected.remaining;
                let before = serde_json::to_vec(&block).unwrap();
                assert_eq!(
                    serde_json::to_vec(&actual.live_block(&block)).unwrap(),
                    serde_json::to_vec(&original(&mut expected, &block)).unwrap(),
                    "remaining {remaining}, encoded {encoded}, required {required}"
                );
                assert_eq!(actual.remaining, expected.remaining);
                assert_eq!(actual.required, expected.required);
                assert_eq!(actual.evicted, expected.evicted);
                assert_eq!(serde_json::to_vec(&block).unwrap(), before);
            }
        }
    }
}

#[test]
fn live_capture_uses_one_encoding_and_one_byte_parse() {
    for block in variants("lexical 水\n {\"x\": 1e+003") {
        take_item_work();
        let mut baseline = Capture::new(1);
        let expected = original(&mut baseline, &block).unwrap();
        assert_eq!(take_item_work(), [2, 2, 1]);
        let mut actual = Capture::new(1);
        assert_eq!(actual.live_block(&block).unwrap(), expected);
        assert_eq!(take_item_work(), [1, 1, 0]);
    }
}

#[test]
fn live_capture_preserves_first_stage_rejection_and_shared_budget_eviction() {
    let mut capture = Capture::new(1);
    capture.remaining = 0;
    let oversized = InFlightBlock::Text {
        block_index: ContentBlockIndex::new(0),
        text: "x".repeat(CHECKPOINT_ITEM_BYTES),
    };
    assert!(capture.live_block(&oversized).is_none());
    assert!(!capture.evicted);
    assert_eq!(capture.required, 0);
    let admitted_item = InFlightBlock::Text {
        block_index: ContentBlockIndex::new(1),
        text: "small".into(),
    };
    assert!(capture.live_block(&admitted_item).is_none());
    assert!(capture.evicted);
    assert_eq!(capture.remaining, 0);
    assert_eq!(capture.required, CHECKPOINT_TARGET_BYTES / 8);
}
