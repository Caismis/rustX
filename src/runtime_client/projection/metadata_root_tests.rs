use crate::runtime_client::content::{
    CanonicalContentComponent, CanonicalContentLocator, ContentRef, ContentRepresentation,
    ContentSource,
};
use std::sync::atomic::{AtomicUsize, Ordering};

fn reference(position: u64) -> ContentRef {
    ContentRef {
        conversation_id: ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18"),
        source: ContentSource::CanonicalMessageBlock {
            component: CanonicalContentComponent::Payload,
            locator: CanonicalContentLocator::ByLedgerPosition { position },
            block_index: ContentBlockIndex::new(0),
        },
        representation: ContentRepresentation::Utf8Text,
        version: 1,
        content_revision: 1,
        readable_end: position,
    }
}

fn leaf_pins(root: &super::super::ContentMetadata) -> usize {
    root.values().map(Arc::strong_count).sum()
}

#[test]
fn retained_checkpoints_share_capability_capture_until_its_owner_changes() {
    let (projection, _store) = initialized_source_projection();
    let seed = projection.checkpoint_seed();
    let first = seed.checkpoint_candidate().unwrap();
    let original = serde_json::to_vec(&first.snapshot.capabilities).unwrap();
    let repeated = seed.checkpoint_candidate().unwrap();
    assert!(Arc::ptr_eq(
        &first.snapshot.capabilities,
        &repeated.snapshot.capabilities
    ));

    let mut changed = projection.capture_working_seed();
    let mut header = changed.capability_head.header();
    header.revision = crate::runtime::identity::CapabilityRevision::new(42);
    changed.capability_head =
        crate::runtime_client::capability_source::PreparedCapabilityHead::new(&header, Vec::new());
    changed.snapshot.capabilities = Arc::new(header);
    changed.prepared =
        super::super::prepared_capture::PreparedCaptures::inherit(&changed, Some(&seed));
    let updated = changed.checkpoint_candidate().unwrap();
    assert!(!Arc::ptr_eq(
        &first.snapshot.capabilities,
        &updated.snapshot.capabilities
    ));
    assert_eq!(updated.snapshot.capabilities.revision.get(), 42);
    assert_eq!(
        serde_json::to_vec(&first.snapshot.capabilities).unwrap(),
        original
    );
}

#[test]
fn issued_metadata_roots_do_not_clone_historical_leaves_and_survive_working_eviction() {
    let (mut projection, _store) = initialized_source_projection();
    projection.install_content_metadata((1..=4096).map(reference).collect());
    projection.initialize_publication_view();
    let original = projection.checkpoint_seed();
    let original_checkpoint = original.checkpoint_candidate().unwrap();
    let first = projection.content_metadata.keys().next().unwrap().clone();
    let evicted = Arc::downgrade(projection.content_metadata.get(&first).unwrap());
    let baseline = leaf_pins(&projection.content_metadata);
    // Independent linear-copy oracle measures the old operation on actual
    // metadata pins, rather than using elapsed time or a simulated Clone.
    let linear: std::collections::BTreeMap<_, _> = projection
        .content_metadata
        .iter()
        .map(|(key, value)| (key.clone(), Arc::clone(value)))
        .collect();
    assert_eq!(leaf_pins(&projection.content_metadata) - baseline, 4096);
    drop(linear);
    let captured = projection.capture_working_seed();
    assert!(
        captured
            .content_metadata
            .ptr_eq(&projection.content_metadata)
    );
    assert_eq!(leaf_pins(&projection.content_metadata), baseline);
    drop(captured);
    projection.install_content_metadata(vec![reference(4097)]);
    assert_eq!(projection.content_metadata.len(), 4096);
    assert!(!projection.content_metadata.contains_key(&first));
    assert!(original.content_metadata.contains_key(&first));
    assert!(evicted.upgrade().is_some());
    assert_eq!(
        original.checkpoint_candidate().unwrap(),
        original_checkpoint
    );
    publish_source_fact(&mut projection, |projection| {
        projection.publish(RuntimeClientEvent::TraceChanged);
    });
    assert!(projection.checkpoint_read_cut().revision > original_checkpoint.read_cut.revision);
    assert_eq!(
        original.checkpoint_candidate().unwrap(),
        original_checkpoint
    );
    drop(original);
    assert!(
        evicted.upgrade().is_none(),
        "the last old root releases its evicted leaf"
    );
}

fn context_source(count: u64) -> Arc<super::super::super::domain::DomainSource> {
    Arc::new(super::super::super::domain::DomainSource::Context(
        Arc::new(super::super::super::domain::SourceValue::new(Arc::new(
            super::super::super::snapshot::RuntimeClientContextView {
                compaction_count: count,
                ..Default::default()
            },
        ))),
    ))
}

#[test]
fn published_source_root_keeps_exact_owner_while_working_root_changes_and_evicts() {
    let (mut projection, _store) = initialized_source_projection();
    let original = context_source(1);
    projection.domain_revisions.insert("context".into(), 1);
    projection.install_domain_sources(vec![(
        "context".into(),
        Arc::clone(&original),
        original.encoded_weight(),
    )]);
    projection.initialize_publication_view();
    let issued = projection.published_sources.as_ref().unwrap().clone();
    let old_cut = projection.checkpoint_read_cut();
    assert!(issued.ptr_eq(&projection.domain_sources));
    assert_eq!(Arc::strong_count(&original), 2);
    let replacement = context_source(2);
    projection.domain_revisions.insert("context".into(), 2);
    projection.install_domain_sources(vec![(
        "context".into(),
        Arc::clone(&replacement),
        replacement.encoded_weight(),
    )]);
    assert!(Arc::ptr_eq(
        &projection.domain_source("context", 0).unwrap(),
        &original
    ));
    for index in 0..32 {
        let source = context_source(index);
        projection.install_domain_sources(vec![(
            format!("agents.{index}"),
            Arc::clone(&source),
            source.encoded_weight(),
        )]);
    }
    assert!(projection.domain_source_bytes <= 16 * 1024 * 1024);
    assert!(projection.domain_sources.len() < 32);
    assert!(Arc::ptr_eq(&issued.get("context").unwrap().0, &original));
    assert_eq!(projection.checkpoint_read_cut(), old_cut);
    publish_source_fact(&mut projection, |projection| {
        projection.publish(RuntimeClientEvent::TraceChanged);
    });
    assert!(Arc::ptr_eq(
        &projection.domain_source("context", 0).unwrap(),
        &replacement
    ));
    let published = projection.published_sources.as_ref().unwrap().clone();
    let unpublished = context_source(3);
    projection.domain_revisions.insert("context".into(), 3);
    projection.install_domain_sources(vec![(
        "context".into(),
        Arc::clone(&unpublished),
        unpublished.encoded_weight(),
    )]);
    projection.publication_failed(&RuntimeClientError::ObservationUnavailable {
        message: "fixture encoding failure after source preparation".into(),
    });
    assert!(
        projection
            .published_sources
            .as_ref()
            .unwrap()
            .ptr_eq(&published)
    );
    assert!(Arc::ptr_eq(&issued.get("context").unwrap().0, &original));
    assert!(Arc::ptr_eq(
        &projection.domain_source("context", 0).unwrap(),
        &replacement
    ));
}

struct Counted {
    value: usize,
    clones: Arc<AtomicUsize>,
    drops: Arc<AtomicUsize>,
}
impl Clone for Counted {
    fn clone(&self) -> Self {
        self.clones.fetch_add(1, Ordering::Relaxed);
        Self {
            value: self.value,
            clones: Arc::clone(&self.clones),
            drops: Arc::clone(&self.drops),
        }
    }
}
impl Drop for Counted {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn metadata_root_updates_clone_and_release_only_changed_paths() {
    for size in [1024, 4096] {
        let clones = Arc::new(AtomicUsize::new(0));
        let drops = Arc::new(AtomicUsize::new(0));
        let value = |value| Counted {
            value,
            clones: Arc::clone(&clones),
            drops: Arc::clone(&drops),
        };
        // This is the exact persistent collection used by both production
        // metadata roots, with counters on every value Clone/Drop callback.
        let mut working = imbl::OrdMap::new();
        for index in 0..size {
            working.insert(index, value(index));
        }
        clones.store(0, Ordering::Relaxed);
        drops.store(0, Ordering::Relaxed);
        let issued = working.clone();
        assert_eq!(clones.load(Ordering::Relaxed), 0);
        working.insert(size, value(size));
        working.remove(&(size / 2));
        let copied = clones.load(Ordering::Relaxed);
        assert!(
            copied > 0 && copied < size / 2,
            "changed-path clones={copied}, rows={size}"
        );
        assert_eq!(issued.len(), size);
        assert_eq!(issued.get(&(size / 2)).unwrap().value, size / 2);
        assert!(!issued.contains_key(&size));
        assert_eq!(working.len(), size);
        assert!(!working.contains_key(&(size / 2)));
        assert_eq!(working.get(&size).unwrap().value, size);
        let before_drop = drops.load(Ordering::Relaxed);
        drop(working);
        assert!(drops.load(Ordering::Relaxed) - before_drop < size / 2);
        drop(issued);
        assert_eq!(drops.load(Ordering::Relaxed), size + 1 + copied);
    }
}

#[test]
fn auxiliary_metadata_seed_maps_share_roots_and_keep_issued_cuts() {
    use crate::runtime::identity::ObservationCollectionCut;
    use crate::runtime_client::checkpoint::{CollectionFrontier, MetadataFragment};
    let (mut projection, _store) = initialized_source_projection();
    for index in 0..128 {
        let domain = format!("auxiliary-{index:03}");
        projection.source_frontiers.insert(
            domain.clone(),
            CollectionFrontier {
                domain: domain.clone(),
                revision: 1,
                total: index,
                loaded: index,
                next_offset: None,
            },
        );
        projection.collection_cuts.insert(
            domain.clone().into(),
            ObservationCollectionCut {
                generation: 1,
                total: index,
            },
        );
        projection.domain_revisions.insert(domain.clone().into(), index);
        projection
            .head_bindings
            .insert(MessageId::new(domain), Vec::new().into());
    }
    let seed = projection.capture_working_seed();
    let old_wire = serde_json::to_vec(&seed.read_cut).unwrap();
    assert!(seed.head_bindings.ptr_eq(&projection.head_bindings));
    assert!(seed.source_frontiers.ptr_eq(&projection.source_frontiers));
    assert!(seed.live_frontiers.ptr_eq(&projection.live_frontiers));
    assert!(seed.collection_cuts.same_owner(&projection.collection_cuts));
    assert!(
        seed.domain_revisions
            .same_owner(&projection.domain_revisions)
    );
    assert!(
        seed.read_cut
            .collection_cuts
            .same_owner(&projection.collection_cuts)
    );
    assert!(
        seed.read_cut
            .domain_revisions
            .same_owner(&projection.domain_revisions)
    );
    let unchanged = projection.capture_working_seed();
    assert!(
        seed.read_cut
            .collection_cuts
            .same_owner(&unchanged.read_cut.collection_cuts)
    );
    projection.collection_cuts.insert(
        "auxiliary-000".into(),
        ObservationCollectionCut {
            generation: 2,
            total: 999,
        },
    );
    projection
        .domain_revisions
        .insert("auxiliary-000".into(), 999);
    projection
        .source_frontiers
        .get_mut("auxiliary-000")
        .unwrap()
        .total = 999;
    projection
        .head_bindings
        .remove(&MessageId::new("auxiliary-000"));
    let updated = projection.capture_working_seed();
    assert!(!seed.collection_cuts.same_owner(&updated.collection_cuts));
    assert!(!seed.domain_revisions.same_owner(&updated.domain_revisions));
    assert!(!seed.source_frontiers.ptr_eq(&updated.source_frontiers));
    assert!(
        seed.head_bindings
            .contains_key(&MessageId::new("auxiliary-000"))
    );
    assert_eq!(seed.source_frontiers.get("auxiliary-000").unwrap().total, 0);
    assert_eq!(
        updated
            .read_cut
            .collection_cuts
            .get("auxiliary-000")
            .unwrap()
            .total,
        999
    );
    assert_eq!(serde_json::to_vec(&seed.read_cut).unwrap(), old_wire);
    // Replacing a current owner must not change an issued root either.
    projection.collection_cuts = MetadataFragment::default();
    assert_eq!(serde_json::to_vec(&seed.read_cut).unwrap(), old_wire);
}

#[test]
fn indexed_source_frontier_merge_preserves_linear_oracle_with_duplicate_domains() {
    use crate::runtime_client::checkpoint::CollectionFrontier;
    for sources_len in [0, 1, 2, 17, 128] {
        for current_len in [0, 1, 4, 65] {
            let sources: imbl::OrdMap<String, CollectionFrontier> = (0..sources_len)
                .map(|index| {
                    (
                        format!("key-{index:03}"),
                        CollectionFrontier {
                            domain: format!("domain-{}", index % 13),
                            revision: index,
                            total: 100 + index,
                            loaded: index % 7,
                            next_offset: (index % 2 == 0).then_some(index),
                        },
                    )
                })
                .collect();
            let current: Vec<_> = (0..current_len)
                .map(|index| CollectionFrontier {
                    domain: format!("domain-{}", index * 7 % 17),
                    revision: u64::MAX,
                    total: 9,
                    loaded: index % 5,
                    next_offset: Some(77),
                })
                .collect();
            let mut expected = current.clone();
            // Original ordered, first-match implementation is the oracle.
            for original in sources.values() {
                if let Some(frontier) = expected.iter_mut().find(|f| f.domain == original.domain) {
                    frontier.total = original.total;
                    frontier.next_offset = if frontier.loaded < original.loaded {
                        Some(0)
                    } else {
                        original.next_offset
                    };
                } else {
                    expected.push(original.clone());
                }
            }
            let mut actual = current;
            super::super::merge_source_frontiers(&sources, &mut actual);
            assert_eq!(
                actual, expected,
                "sources={sources_len},current={current_len}"
            );
        }
    }
}
