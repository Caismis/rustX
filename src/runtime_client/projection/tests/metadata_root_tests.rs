use super::*;

use crate::observation::content::{
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
    projection.domain_revisions.insert("context", 1);
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
    projection.domain_revisions.insert("context", 2);
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
    projection.domain_revisions.insert("context", 3);
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
