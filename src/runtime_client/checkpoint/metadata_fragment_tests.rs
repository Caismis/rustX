use super::*;
use crate::runtime::identity::{AttemptId, ConversationId, MessageId};
use crate::runtime_client::{content::*, event::RuntimeClientEvent, preflight, structural_json};

fn binding() -> ContentBinding {
    ContentBinding {
        source_path: Some(vec!["text\"\\\n水".into()]),
        path: vec!["messages".into(), "0".into(), "content".into(), "0".into()],
        content_ref: ContentRef {
            conversation_id: ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18"),
            source: ContentSource::CanonicalMessageBlock {
                component: CanonicalContentComponent::Payload,
                locator: CanonicalContentLocator::ByMessageId {
                    message_id: MessageId::new("m\"\\\n水"),
                },
                block_index: crate::message::types::ContentBlockIndex::new(0),
            },
            representation: ContentRepresentation::Utf8Text,
            version: 1,
            content_revision: 1,
            readable_end: 99,
        },
        deferred: true,
    }
}
fn metadata() -> ProjectionMetadata {
    ProjectionMetadata {
        read_cut: CheckpointReadCut {
            canonical_foreground: None,
            live_publication: None,
            conversation_id: binding().content_ref.conversation_id,
            activation_id: None,
            revision: 9,
            through: 99,
            source_cut: None,
            domain_revisions: std::collections::BTreeMap::new(),
            collection_cuts: std::collections::BTreeMap::new(),
        },
        collections: Vec::new(),
        content: Vec::new(),
    }
}
fn encoded<T: Serialize + schemars::JsonSchema>(value: &T) -> serde_json::Result<Vec<u8>> {
    let root = structural_json::root::<T>();
    serde_json::to_vec(&structural_json::Encoded {
        value,
        schema: &root,
        root: &root,
    })
}
fn compare(metadata: ProjectionMetadata, count: usize) {
    let events = (0..count)
        .map(|turn| RuntimeClientEvent::AttemptTurnUpdated {
            attempt_id: AttemptId::new("attempt\"\\\n水"),
            turn: u32::try_from(turn).unwrap(),
        })
        .collect::<Vec<_>>();
    let events_budget = preflight::required_safe_integer_budget(&events, CHECKPOINT_TARGET_BYTES);
    let event = RuntimeClientEvent::ProjectionTransition { events, metadata };
    let budget = preflight::required_safe_integer_budget(&event, CHECKPOINT_TARGET_BYTES);
    let bytes = encoded(&event).ok();
    let mut limits = vec![0, 1, 128, CHECKPOINT_TARGET_BYTES];
    if let Some(bytes) = &bytes {
        limits.extend([bytes.len() - 1, bytes.len(), bytes.len() + 1]);
    }
    if let Some(budget) = budget {
        limits.extend([budget - 1, budget, budget + 1]);
    }
    for limit in limits {
        let original = bounded_structural_len_with_budget(&event, limit, budget);
        let actual = metadata_fragment::transition_length(&event, events_budget, budget, limit);
        assert_eq!(actual, original, "limit={limit}, count={count}");
        if let Some(actual) = actual {
            assert_eq!(Some(actual), bytes.as_ref().map(Vec::len));
        }
    }
    let RuntimeClientEvent::ProjectionTransition { metadata, .. } = event else {
        unreachable!()
    };
    let direct = preflight::required_safe_integer_budget(&metadata, CHECKPOINT_TARGET_BYTES);
    assert_eq!(
        metadata
            .fragment_budget()
            .filter(|budget| *budget <= CHECKPOINT_TARGET_BYTES),
        direct,
        "fragment charge exactly matches original visitor"
    );
}

#[test]
fn immutable_metadata_fragments_preserve_schema_wire_and_exact_budget_boundaries() {
    assert_eq!(
        serde_json::to_value(schemars::schema_for!(MetadataFragment<ContentBinding>)).unwrap(),
        serde_json::to_value(schemars::schema_for!(ContentBinding)).unwrap()
    );
    assert_eq!(
        serde_json::to_value(schemars::schema_for!(MetadataFragment<CollectionFrontier>)).unwrap(),
        serde_json::to_value(schemars::schema_for!(CollectionFrontier)).unwrap()
    );
    for count in [0, 1, 5] {
        let mut value = metadata();
        for index in 0..count {
            let mut content = binding();
            content.source_path = (index % 2 == 0).then(|| vec!["text\"\\\n水".into()]);
            content.path.push(index.to_string());
            let fragment: MetadataFragment<_> = content.clone().into();
            assert_eq!(encoded(&fragment).unwrap(), encoded(&content).unwrap());
            let decoded: MetadataFragment<ContentBinding> =
                serde_json::from_slice(&serde_json::to_vec(&fragment).unwrap()).unwrap();
            assert_eq!(decoded, fragment);
            value.content.push(fragment);
            value.collections.push(
                CollectionFrontier {
                    domain: format!("messages.{index}\"\\\n水"),
                    revision: 9,
                    total: 10,
                    loaded: 3,
                    next_offset: (index % 2 == 0).then_some(3),
                }
                .into(),
            );
        }
        for cursor in [9, 10, 99, 100, 9_007_199_254_740_991, u64::MAX] {
            value.read_cut.revision = cursor;
            compare(value.clone(), count);
        }
    }
}

#[test]
fn immutable_metadata_fragments_preserve_invalid_and_oversized_admission() {
    for revision in [9_007_199_254_740_991, 9_007_199_254_740_992, u64::MAX] {
        let mut value = metadata();
        let mut content = binding();
        content.content_ref.content_revision = revision;
        value.content.push(content.into());
        compare(value, 2);
    }
    for length in [
        CHECKPOINT_TARGET_BYTES - 1024,
        CHECKPOINT_TARGET_BYTES,
        CHECKPOINT_TARGET_BYTES + 1,
    ] {
        let mut value = metadata();
        let mut content = binding();
        content.path.push("x".repeat(length));
        value.content.push(content.into());
        compare(value, 1);
    }
}

#[test]
fn immutable_metadata_resolution_keeps_old_owner_and_measures_each_value_once() {
    let base: MetadataFragment<_> = binding().into();
    metadata_fragment::take_work();
    let initial = base.size();
    let initial_bytes = encoded(&base).unwrap();
    assert_eq!(metadata_fragment::take_work(), 1);
    let mut reference = base.content_ref.clone();
    reference.content_revision = 10;
    reference.readable_end = 1024;
    let resolved = base.resolve(&reference);
    let resolved_bytes = encoded(&resolved).unwrap();
    assert!(!base.same_owner(&resolved));
    for _ in 0..32 {
        assert_eq!(base.size().exact, initial.exact);
        let replay = base.resolve(&reference);
        assert!(replay.same_owner(&resolved));
        assert_eq!(replay.size().exact, Some(resolved_bytes.len()));
    }
    assert_eq!(
        metadata_fragment::take_work(),
        1,
        "only the first resolved owner is measured"
    );
    reference.content_revision = u64::MAX;
    assert!(base.resolve(&reference).size().exact.is_none());
    assert_eq!(encoded(&base).unwrap(), initial_bytes);
    assert_eq!(encoded(&resolved).unwrap(), resolved_bytes);
    assert!(base.resolve(&base.content_ref).same_owner(&base));
}

#[test]
fn prepared_capture_reuses_metadata_value_and_exact_length_without_deep_copy() {
    let owner = std::sync::OnceLock::new();
    let mut previous = None;
    metadata_fragment::take_work();
    for revision in 1..33 {
        let mut capture = Capture::new(revision);
        capture.prepared(&owner, |capture| {
            capture.content.push(binding().into());
            "finite unchanged body".to_owned()
        });
        let fragment = &capture.content[0];
        if let Some(previous) = &previous {
            assert!(fragment.same_owner(previous));
        }
        assert_eq!(
            fragment.size().exact,
            Some(encoded(fragment).unwrap().len())
        );
        previous = Some(fragment.clone());
    }
    assert_eq!(
        metadata_fragment::take_work(),
        1,
        "capture replay neither clones nor remeasures metadata"
    );
}
