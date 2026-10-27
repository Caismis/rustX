use super::*;
use std::{cell::Cell, sync::OnceLock};

#[test]
fn incremental_capture_replays_the_exact_shared_budget_at_every_item_boundary() {
    let source = vec!["water 水 \"\n".repeat(73); 5];
    let owner = OnceLock::new();
    let builds = Cell::new(0);
    for budget in 0..=CHECKPOINT_ITEM_BYTES {
        let mut expected = Capture::new(99);
        expected.remaining = budget;
        let wanted = expected.page("rows", &source);
        let mut actual = Capture::new(99);
        actual.remaining = budget;
        let (value, _) = actual.prepared(&owner, |capture| {
            builds.set(builds.get() + 1);
            capture.page("rows", &source)
        });
        assert_eq!(value.as_ref(), &wanted, "remaining {budget}");
        assert_eq!(actual.remaining, expected.remaining, "remaining {budget}");
        assert_eq!(actual.evicted, expected.evicted, "remaining {budget}");
        assert_eq!(
            actual.collections, expected.collections,
            "remaining {budget}"
        );
        assert_eq!(actual.content, expected.content, "remaining {budget}");
    }
    let before = builds.get();
    for revision in 100..200 {
        let mut capture = Capture::new(revision);
        let (value, _) = capture.prepared(&owner, |capture| {
            builds.set(builds.get() + 1);
            capture.page("rows", &source)
        });
        assert_eq!(value.as_ref(), &source);
        assert_eq!(capture.collections[0].revision, revision);
    }
    assert_eq!(
        builds.get(),
        before,
        "unchanged heads perform no normalization"
    );
}

#[test]
fn incremental_capture_keeps_invalid_rows_and_optional_body_eviction_exact() {
    for source in [
        vec![serde_json::json!([])],
        vec![serde_json::Value::Array(vec![serde_json::Value::Null; 33])],
        vec![serde_json::json!({"huge": "x".repeat(CHECKPOINT_ITEM_BYTES + 1)})],
        vec![serde_json::json!({"wide": u64::MAX})],
        vec![
            serde_json::json!({"valid": "水🙂"}),
            serde_json::json!("x".repeat(20_000)),
        ],
    ] {
        let owner = OnceLock::new();
        for budget in [
            0,
            1,
            31,
            512,
            16_383,
            16_384,
            32_768,
            CHECKPOINT_TARGET_BYTES / 8,
        ] {
            let mut expected = Capture::new(7);
            expected.remaining = budget;
            let wanted = expected.page("rows", &source);
            let mut actual = Capture::new(7);
            actual.remaining = budget;
            let (value, _) = actual.prepared(&owner, |capture| capture.page("rows", &source));
            assert_eq!(value.as_ref(), &wanted);
            assert_eq!(actual.remaining, expected.remaining);
            assert_eq!(actual.evicted, expected.evicted);
            assert_eq!(actual.collections, expected.collections);
        }
    }
}

#[test]
fn prepared_capture_borrows_cold_and_warm_domains_until_materialized() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    #[derive(serde::Serialize)]
    struct Tracked {
        body: Vec<String>,
        #[serde(skip)]
        clones: Arc<AtomicUsize>,
    }
    impl Clone for Tracked {
        fn clone(&self) -> Self {
            self.clones.fetch_add(1, Ordering::Relaxed);
            Self {
                body: self.body.clone(),
                clones: self.clones.clone(),
            }
        }
    }
    let clones = Arc::new(AtomicUsize::new(0));
    let owner = OnceLock::new();
    let rows = vec!["borrowed immutable 水\n".repeat(300)];
    let build = |capture: &mut Capture| Tracked {
        body: capture.page("rows", &rows),
        clones: clones.clone(),
    };
    for revision in 0..4 {
        let mut capture = Capture::new(revision);
        let (value, _) = capture.prepared(&owner, build);
        assert!(matches!(value, std::borrow::Cow::Borrowed(_)));
        assert_eq!(value.body, rows);
        assert_eq!(
            clones.load(Ordering::Relaxed),
            0,
            "even cold normalization returns its immutable value without a copy"
        );
    }
    let mut pressured = Capture::new(4);
    pressured.remaining = 0;
    let (value, _) = pressured.prepared(&owner, build);
    assert!(matches!(value, std::borrow::Cow::Owned(_)));
    assert!(value.into_owned().body.is_empty());
    assert_eq!(
        clones.load(Ordering::Relaxed),
        0,
        "a pressured admission already owns its result"
    );
    let mut capture = Capture::new(5);
    let (value, _) = capture.prepared(&owner, build);
    assert_eq!(value.into_owned().body, rows);
    assert_eq!(
        clones.load(Ordering::Relaxed),
        1,
        "only explicit snapshot materialization copies a borrowed domain"
    );
}
