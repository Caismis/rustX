use super::{SelectedInputManifest, SelectedModelInput};
use crate::message::content::TextBlock;
use crate::message::types::{
    InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use crate::model::ModelInputMessage;
use crate::runtime::identity::{ConversationId, MessageId};
use std::sync::Arc;

fn message(index: usize) -> ModelInputMessage {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new(format!("selected-request-{index}")),
        source: UserSource::Human,
        kind: InboundKind::Message,
        content: vec![UserContentBlock::Text(TextBlock {
            text: format!("original 水🙂 {index}"),
        })],
        timestamp: None,
    })
    .into()
}

#[test]
fn cold_request_values_keep_exact_flat_wire_and_only_scoped_freeze_creates_evidence() {
    let flat: Vec<_> = (0..17).map(message).collect();
    let input: SelectedModelInput = flat.clone().into();
    assert_eq!(
        serde_json::to_vec(&input).unwrap(),
        serde_json::to_vec(&flat).unwrap()
    );
    assert_eq!(input.0.root.as_ref().unwrap().frozen_count, 0);
    let decoded: SelectedModelInput =
        serde_json::from_slice(&serde_json::to_vec(&input).unwrap()).unwrap();
    assert!(input.same_input(&decoded));
    assert_eq!(decoded.0.root.as_ref().unwrap().frozen_count, 0);
    let scope = ConversationId::generate();
    let frozen = input.freeze(&scope);
    assert_eq!(frozen.root.as_ref().unwrap().frozen_count, flat.len());
    assert_eq!(frozen, SelectedInputManifest::from_messages(&scope, &flat));
    assert!(frozen.messages().same_input(&input));
}

#[test]
fn selected_request_clone_ranges_and_mutations_keep_original_root_and_exact_values() {
    let scope = ConversationId::generate();
    let flat: Vec<_> = (0..100).map(message).collect();
    let frozen = SelectedInputManifest::from_messages(&scope, &flat);
    let original = frozen.messages();
    let mut changed = original.clone();
    assert!(Arc::ptr_eq(
        original.0.root.as_ref().unwrap(),
        changed.0.root.as_ref().unwrap()
    ));
    assert_eq!(original.range(21..73).to_vec(), flat[21..73]);
    assert_eq!(
        original.iter_rev().cloned().collect::<Vec<_>>(),
        flat.iter().rev().cloned().collect::<Vec<_>>()
    );
    changed[50] = message(999);
    assert!(!original.same_input(&changed));
    assert_eq!(original.to_vec(), flat);
    assert_eq!(changed[49], original[49]);
    assert_eq!(changed[50], message(999));
    assert_eq!(changed.0.root.as_ref().unwrap().frozen_count, 0);
    let rebound = changed.freeze(&scope);
    assert_eq!(rebound.root.as_ref().unwrap().frozen_count, flat.len());
    assert_eq!(
        rebound,
        SelectedInputManifest::from_messages(&scope, &changed)
    );
}

#[test]
fn shared_selected_iterator_mixed_ends_ranges_and_repeated_occurrences_match_flat_order() {
    use std::collections::VecDeque;

    let flat: Vec<_> = (0..17)
        .map(|index| {
            let mut value = message(index % 5);
            if index.is_multiple_of(3)
                && let ModelInputMessage::Canonical(MessageBlock::User(user)) = &mut value
            {
                user.id = MessageId::new("same-id-distinct-values");
            }
            value
        })
        .collect();
    let input: SelectedModelInput = flat.clone().into();
    for interval in [0..0, 0..1, 0..8, 0..9, 3..12, 2..10, 5..6, 17..17] {
        let selected = input.range(interval.clone());
        let expected = &flat[interval];
        for pattern in 0..(1usize << expected.len()) {
            let mut oracle: VecDeque<_> = expected.iter().collect();
            let mut actual = selected.iter();
            for step in 0..expected.len() {
                assert_eq!(actual.len(), oracle.len());
                assert_eq!(actual.size_hint(), (oracle.len(), Some(oracle.len())));
                assert_eq!(
                    actual.clone().collect::<Vec<_>>(),
                    oracle.iter().copied().collect::<Vec<_>>()
                );
                let (observed, wanted) = if pattern & (1 << step) == 0 {
                    (actual.next(), oracle.pop_front())
                } else {
                    (actual.next_back(), oracle.pop_back())
                };
                assert_eq!(observed, wanted);
            }
            assert_eq!(actual.len(), 0);
            assert_eq!(actual.size_hint(), (0, Some(0)));
            assert!(actual.next().is_none());
            assert!(actual.next_back().is_none());
            assert!(actual.next().is_none());
            assert!(actual.next_back().is_none());
        }
    }
}
