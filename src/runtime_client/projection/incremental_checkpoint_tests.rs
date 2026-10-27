use crate::durable::ConversationStore;
use crate::runtime_client::{checkpoint, preflight};

fn compare_original(seed: &CheckpointSeed) {
    let (mut original, eviction) = seed.checkpoint_unfitted().unwrap();
    let (mut actual, actual_eviction, budget) = seed.checkpoint_prepared().unwrap();
    assert_eq!(actual_eviction, eviction);
    assert_eq!(
        serde_json::to_vec(&actual).unwrap(),
        serde_json::to_vec(&original).unwrap()
    );
    if let Some(budget) = budget {
        let direct =
            preflight::required_safe_integer_budget(&actual, checkpoint::CHECKPOINT_TARGET_BYTES);
        assert!(
            direct.is_some_and(|direct| direct <= budget),
            "additive certificate is conservative"
        );
    }
    let original_fitted = checkpoint::fit_checkpoint(&mut original).unwrap();
    let actual_fitted = checkpoint::fit_checkpoint_with_budget(&mut actual, budget).unwrap();
    assert_eq!(actual_fitted, original_fitted);
    assert_eq!(
        serde_json::to_vec(&actual).unwrap(),
        serde_json::to_vec(&original).unwrap()
    );
}

#[test]
fn incremental_checkpoint_reuses_unchanged_domains_without_capture_or_whole_fit() {
    let (mut projection, store) = initialized_source_projection();
    for index in 0..8 {
        let mut message = compactable_user(&format!("prepared-message-{index}"));
        let MessageBlock::User(user) = &mut message else {
            unreachable!()
        };
        user.content = vec![crate::message::types::UserContentBlock::Text(
            crate::message::content::TextBlock {
                text: "immutable 水\n".repeat(40),
            },
        )];
        let receipt = store.append_canonical(&message).unwrap();
        publish_source_fact(&mut projection, |projection| {
            projection.apply(ConversationObservation::CanonicalCommitted {
                attempt_id: None,
                receipt,
            });
        });
    }
    let historical = projection.checkpoint_seed();
    let old = historical.checkpoint_candidate().unwrap();
    checkpoint::take_capture_work();
    publish_source_fact(&mut projection, |projection| {
        projection.publish(RuntimeClientEvent::TraceChanged);
    });
    let current = projection.checkpoint_seed();
    let work = checkpoint::take_capture_work();
    assert_eq!(work[0], 0, "no immutable domain recapture: {work:?}");
    assert!(work[1] >= 19, "prepared domains replay admission: {work:?}");
    assert_eq!(work[2], 0, "normal workset stays inside certified budgets");
    assert_eq!(work[3], 0, "no complete checkpoint fit walk");
    assert!(current.prepared.shared(&historical.prepared, "messages"));
    compare_original(&current);
    assert_eq!(
        historical.checkpoint_candidate().unwrap(),
        old,
        "held cut remains immutable"
    );
}

#[test]
fn incremental_checkpoint_changed_domains_and_cursor_widths_match_original_bytes() {
    let (mut projection, store) = initialized_source_projection();
    for revision in [9, 99, 999, 9_007_199_254_740_990] {
        projection.cursor = RuntimeClientCursor::new(revision);
        projection.initialize_publication_view();
        let before = projection.checkpoint_seed();
        before.checkpoint_candidate().unwrap();
        let receipt = store
            .append_canonical(&compactable_user(&format!("cut-{revision}")))
            .unwrap();
        publish_source_fact(&mut projection, |projection| {
            projection.apply(ConversationObservation::CanonicalCommitted {
                attempt_id: None,
                receipt,
            });
            projection.snapshot.context.compaction_count += 1;
            projection.snapshot.model.as_mut().unwrap().unavailable =
                Some(format!("model-{revision}"));
        });
        let seed = projection.checkpoint_seed();
        assert!(!seed.prepared.shared(&before.prepared, "messages"));
        assert!(!seed.prepared.shared(&before.prepared, "context"));
        assert!(!seed.prepared.shared(&before.prepared, "model"));
        compare_original(&seed);
        compare_original(&before);
    }
}

#[test]
fn incremental_checkpoint_budget_pressure_and_unsafe_quantities_match_original() {
    let (projection, _) = initialized_source_projection();
    for size in [0, 16_000, 16_384, 32_768, 100_000] {
        let mut seed = (*projection.checkpoint_seed()).clone();
        seed.prepared = super::super::prepared_capture::PreparedCaptures::default();
        seed.snapshot.model.as_mut().unwrap().unavailable = Some("x".repeat(size));
        seed.read_cut
            .domain_revisions
            .insert("wide".into(), u64::MAX);
        compare_original(&seed);
        assert!(
            seed.checkpoint_prepared().unwrap().2.is_none(),
            "wide quantities keep schema admission"
        );
    }
}
use crate::runtime_client::projection::CheckpointSeed;

