use super::*;
use crate::conversation::{SurfaceOp, message_id_of};
use crate::durable::LineageSeed;
use crate::local_runtime::session::remap_seed;

#[test]
#[allow(clippy::too_many_lines)] // One three-generation bootstrap/reopen/local-execution contract.
fn deep_lineage_reopen_preserves_response_facts_without_execution_ownership() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = SqliteConversationStore::open(
        ConversationId::new("conv_b05f9cb7-dcec-7fa1-8fa9-2047ae76d95f"),
        &directory.path().join("source.sqlite"),
    )
    .unwrap();
    for attempt in ["a", "b"] {
        user(&store, &format!("input-{attempt}"));
        append(
            &store,
            attempt,
            RuntimeEvent::AttemptStarted {
                attempt_id: AttemptId::new(attempt),
            },
        );
        request(&store, attempt, 0, Some(usage(Some(80))));
        assistant(&store, attempt, &format!("response-{attempt}"));
        finish(&store, attempt);
    }
    let original = page(&store, None, 64);
    let original_b = tails(&original)[1].clone();
    for generation in 1..=3 {
        let id = ConversationId::generate();
        let path = directory.path().join(format!("child-{generation}.sqlite"));
        let canonical = store.load_canonical().unwrap();
        let history = store
            .load_surface_history(store.load_head().unwrap().revision)
            .unwrap();
        let provenance = lineage_provenance(&store, &canonical).unwrap();
        let seed = remap_seed(&id, &canonical, &history, &provenance).unwrap();
        let child = SqliteConversationStore::open(id.clone(), &path).unwrap();
        child.initialize_lineage(&seed).unwrap();
        assert_eq!(child.load_canonical().unwrap().len(), 4);
        assert!(
            child
                .load_canonical()
                .unwrap()
                .iter()
                .zip(&canonical)
                .all(|(a, b)| message_id_of(a) != message_id_of(b))
        );
        let projected = page(&child, None, 64);
        let inherited = tails(&projected);
        assert_eq!(inherited.len(), 2);
        let b = inherited[1];
        assert_eq!(b.closing_message_id, message_id_of(&seed.canonical()[3]));
        assert_eq!(
            b.retry_message_id,
            Some(message_id_of(&seed.canonical()[2]))
        );
        assert_eq!(b.origin, original_b.origin);
        assert_eq!(b.completed_at, original_b.completed_at);
        assert_eq!(b.usage, original_b.usage);
        assert!(is_completed_response(&child, &b.closing_message_id).unwrap());
        assert_eq!(
            projected.statistics,
            Some(ConversationStatistics::default())
        );
        assert_eq!(child.presentation_frontier().unwrap(), 0);
        assert!(child.read_events(None, 128).unwrap().events.is_empty());
        assert!(
            crate::context::occupancy::read(&child, 0)
                .unwrap()
                .is_none()
        );
        let paged = page(&child, None, 1);
        assert_eq!(tails(&paged), vec![b]);
        drop(child);
        store = SqliteConversationStore::open(id, &path).unwrap();
        // Production reopen supplies canonical bootstrap history, never execution residue.
        store
            .initialize(&store.load_bootstrap_history().unwrap())
            .unwrap();
        assert_eq!(page(&store, None, 64), projected);
        let mut changed = seed.completed_responses().to_vec();
        changed[1].usage = None;
        let changed =
            LineageSeed::replayed(seed.canonical().to_vec(), seed.surface_history().to_vec())
                .unwrap()
                .with_completed_responses(changed)
                .unwrap();
        assert!(matches!(
            store.initialize_lineage(&changed),
            Err(ConversationStoreError::InitialHistoryMismatch)
        ));
    }
    // Local execution totals start here, independently of both historical tails.
    user(&store, "local-user");
    append(
        &store,
        "local",
        RuntimeEvent::AttemptStarted {
            attempt_id: AttemptId::new("local"),
        },
    );
    request(&store, "local", 0, Some(usage(None)));
    assistant(&store, "local", "local-response");
    finish(&store, "local");
    let current = page(&store, None, 64);
    assert_eq!(tails(&current).len(), 3);
    let totals = current.statistics.unwrap();
    assert_eq!(totals.completed_responses, 1);
    assert_eq!(totals.model_requests, 1);
    assert_eq!(totals.reported_usage.unwrap().total_tokens, 120);
}

#[test]
fn remapping_drops_missing_retry_input_and_bootstrap_rejects_invalid_response_addresses() {
    let source = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    user(&source, "input");
    request(&source, "attempt", 0, Some(usage(None)));
    assistant(&source, "attempt", "response");
    finish(&source, "attempt");
    let canonical = source.load_canonical().unwrap();
    let provenance = lineage_provenance(&source, &canonical).unwrap();
    let id = ConversationId::generate();
    let seed = remap_seed(
        &id,
        &canonical[1..],
        &[SurfaceOp::Append {
            message_id: MessageId::new("response"),
        }],
        &provenance,
    )
    .unwrap();
    assert_eq!(seed.completed_responses()[0].retry_message_id, None);
    let child = SqliteConversationStore::in_memory(id).unwrap();
    child.initialize_lineage(&seed).unwrap();
    assert_eq!(tails(&page(&child, None, 64))[0].retry_message_id, None);
    let invalid =
        LineageSeed::history(canonical[..1].to_vec()).with_completed_responses(provenance.clone());
    assert!(invalid.is_err());
    let invalid = LineageSeed::history(canonical.clone())
        .with_completed_responses(vec![provenance[0].clone(), provenance[0].clone()]);
    assert!(invalid.is_err());
    let mut wrong_retry = provenance;
    wrong_retry[0].retry_message_id = Some(MessageId::new("response"));
    assert!(
        LineageSeed::history(canonical)
            .with_completed_responses(wrong_retry)
            .is_err()
    );
}
