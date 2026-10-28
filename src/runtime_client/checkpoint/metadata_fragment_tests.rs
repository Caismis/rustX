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
    let arrays = metadata_fragment::MetadataArrays::default().prepare(&metadata);
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
        let actual =
            metadata_fragment::transition_length(&event, events_budget, budget, limit, None);
        assert_eq!(actual, original, "limit={limit}, count={count}");
        assert_eq!(
            metadata_fragment::transition_length(
                &event,
                events_budget,
                budget,
                limit,
                Some(&arrays)
            ),
            original,
            "cached array proof: limit={limit}, count={count}"
        );
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
    assert_eq!(
        arrays.fragment_budget(&metadata),
        metadata.fragment_budget()
    );
}

#[test]
fn metadata_array_cache_requires_ordered_owners_and_preserves_stable_duplicates() {
    let cache = metadata_fragment::MetadataArrays::default();
    let mut value = metadata();
    for (path, revision) in [("z", 1), ("a", 2), ("a", 3)] {
        let mut content = binding();
        content.path = vec![path.into()];
        content.content_ref.content_revision = revision;
        value.content.push(content.into());
    }
    for domain in ["z", "a", "a"] {
        value.collections.push(
            CollectionFrontier {
                domain: domain.into(),
                revision: 1,
                total: 2,
                loaded: 1,
                next_offset: Some(1),
            }
            .into(),
        );
    }
    let original = value.clone();
    let original_bytes = encoded(&original).unwrap();
    metadata_fragment::take_array_work();
    let first = cache.prepare(&value);
    first.sort_into(&mut value);
    assert_eq!(metadata_fragment::take_array_work(), [2, 2, 0]);
    assert_eq!(
        value
            .content
            .iter()
            .map(|binding| binding.content_ref.content_revision)
            .collect::<Vec<_>>(),
        [2, 3, 1]
    );
    for revision in [9, 10, 99, 100, u64::MAX] {
        let mut repeated = original.clone();
        repeated.read_cut.revision = revision;
        let prepared = cache.prepare(&repeated);
        assert_eq!(
            prepared.fragment_budget(&repeated),
            repeated.fragment_budget()
        );
        prepared.sort_into(&mut repeated);
        let mut expected = original.clone();
        expected.read_cut.revision = revision;
        expected.content.sort_by(|a, b| a.path.cmp(&b.path));
        expected.collections.sort_by(|a, b| a.domain.cmp(&b.domain));
        assert_eq!(
            serde_json::to_vec(&repeated).unwrap(),
            serde_json::to_vec(&expected).unwrap()
        );
        compare(repeated, 2);
    }
    // compare() uses a distinct local cache; reset its independent work here.
    metadata_fragment::take_array_work();
    let mut repeated = original.clone();
    cache.prepare(&repeated).sort_into(&mut repeated);
    assert_eq!(metadata_fragment::take_array_work(), [0, 0, 2]);

    let mut moved = original.clone();
    moved.content.swap(1, 2);
    cache.prepare(&moved).sort_into(&mut moved);
    assert_eq!(metadata_fragment::take_array_work(), [1, 1, 1]);
    assert_eq!(
        moved
            .content
            .iter()
            .map(|binding| binding.content_ref.content_revision)
            .collect::<Vec<_>>(),
        [3, 2, 1]
    );

    cache.prepare(&original);
    metadata_fragment::take_array_work();
    let mut new_owner = original.clone();
    new_owner.content[0] = new_owner.content[0].clone().into_owned().into();
    cache.prepare(&new_owner).sort_into(&mut new_owner);
    assert_eq!(metadata_fragment::take_array_work(), [1, 1, 1]);
    assert_eq!(encoded(&new_owner).unwrap(), encoded(&value).unwrap());
    assert_eq!(
        encoded(&original).unwrap(),
        original_bytes,
        "old values remain immutable"
    );
    assert_eq!(first.fragment_budget(&original), original.fragment_budget());
}

#[test]
fn metadata_array_cache_invalidates_changed_authority_without_changing_old_cut() {
    let cache = metadata_fragment::MetadataArrays::default();
    let mut value = metadata();
    value.content.push(binding().into());
    let old = value.clone();
    let old_bytes = encoded(&old).unwrap();
    let prepared = cache.prepare(&old);
    let mut reference = value.content[0].content_ref.clone();
    reference.content_revision = u64::MAX;
    value.content[0] = value.content[0].resolve(&reference);
    metadata_fragment::take_array_work();
    let changed = cache.prepare(&value);
    assert_eq!(metadata_fragment::take_array_work(), [1, 0, 1]);
    assert!(changed.fragment_budget(&value).is_none());
    assert!(prepared.fragment_budget(&old).is_some());
    assert_eq!(encoded(&old).unwrap(), old_bytes);
    compare(value, 1);
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
