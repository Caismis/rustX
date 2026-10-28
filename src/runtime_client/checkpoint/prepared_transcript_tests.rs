use super::*;
use crate::message::content::TextBlock;
use crate::message::types::{AssistantContentBlock, AssistantMessageBlock, MessageBlock};
use crate::runtime::identity::MessageId;
use crate::runtime_client::checkpoint::{CHECKPOINT_ITEM_BYTES, bounded_len};
use crate::runtime_client::snapshot::{RuntimeClientTranscriptCursor, RuntimeClientTranscriptItem};

fn conversation() -> ConversationId {
    ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18")
}
fn entry(cursor: u64, bytes: usize) -> RuntimeClientTranscriptEntry {
    RuntimeClientTranscriptEntry {
        cursor: RuntimeClientTranscriptCursor::new(cursor),
        item: RuntimeClientTranscriptItem::Message {
            message: MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new(format!("message-{cursor}")),
                content: vec![AssistantContentBlock::Text(TextBlock {
                    text: "x".repeat(bytes),
                })],
            }),
        },
        response_pending: false,
        turn_process: None,
        completed_response: None,
        tool_calls: Vec::new(),
    }
}
fn row(cursor: u64, bytes: usize) -> PreparedTranscriptRow {
    PreparedTranscriptRow::new(conversation(), entry(cursor, bytes))
}

// Original page algorithm, independent of the row-owner replay and its index
// mapping. Keep the exact failed-row content truncation and frontier ordering.
fn original(
    capture: &mut Capture,
    entries: &[RuntimeClientTranscriptEntry],
) -> Vec<RuntimeClientTranscriptEntry> {
    let mut result = Vec::new();
    for (index, entry) in entries.iter().take(CHECKPOINT_PAGE_ITEMS).enumerate() {
        let before = capture.content.len();
        let Some(projected) = capture.transcript_entry(
            entry,
            &conversation(),
            &["transcript".into(), "entries".into(), index.to_string()],
        ) else {
            capture.content.truncate(before);
            break;
        };
        let Some(length) = bounded_len(&projected, capture.item_budget()) else {
            capture.content.truncate(before);
            break;
        };
        capture.remaining -= length;
        result.push(projected);
    }
    capture.frontier(
        "transcript.entries",
        entries.len() as u64,
        result.len() as u64,
        (result.len() < entries.len()).then_some(result.len() as u64),
    );
    result
}
fn compare(
    head: &PreparedTranscriptHead,
    entries: &[RuntimeClientTranscriptEntry],
    remaining: usize,
    revision: u64,
) {
    let mut actual = Capture::new(revision);
    actual.remaining = remaining;
    let (rows, budget) = head.capture(&mut actual);
    let rows = rows.into_owned();
    let mut expected = Capture::new(revision);
    expected.remaining = remaining;
    let wanted = original(&mut expected, entries);
    assert_eq!(
        serde_json::to_vec(&rows).unwrap(),
        serde_json::to_vec(&wanted).unwrap()
    );
    assert_eq!(actual.remaining, expected.remaining);
    assert_eq!(actual.required, expected.required);
    assert_eq!(actual.evicted, expected.evicted);
    assert_eq!(
        serde_json::to_vec(&actual.content).unwrap(),
        serde_json::to_vec(&expected.content).unwrap()
    );
    assert_eq!(actual.collections, expected.collections);
    assert_eq!(
        budget,
        crate::runtime_client::preflight::required_safe_integer_budget(
            &wanted,
            CHECKPOINT_TARGET_BYTES
        )
    );
}
fn values(rows: &[PreparedTranscriptRow]) -> Vec<RuntimeClientTranscriptEntry> {
    rows.iter().map(|row| row.value().clone()).collect()
}

#[test]
fn transcript_row_owners_moving_eight_row_head_prepares_only_new_row() {
    let rows: Vec<_> = (1..=8).map(|cursor| row(cursor, 512)).collect();
    let old = PreparedTranscriptHead::new(rows.clone());
    take_work();
    let mut capture = Capture::new(1);
    let retained = old.capture(&mut capture).0.into_owned();
    assert_eq!(take_work(), 8);
    let mut moved = rows[1..].to_vec();
    moved.push(row(9, 512));
    let next = PreparedTranscriptHead::new(moved.clone());
    let mut capture = Capture::new(99);
    let captured = next.capture(&mut capture).0.into_owned();
    assert_eq!(
        take_work(),
        1,
        "seven complete immutable rows retain their normalization"
    );
    assert_eq!(captured.len(), 8);
    compare(&next, &values(&moved), CHECKPOINT_TARGET_BYTES / 8, 99);
    assert_eq!(old.capture(&mut Capture::new(1)).0.into_owned(), retained);
}

#[test]
fn transcript_row_owners_replay_shared_budget_and_failed_admission_exactly() {
    let rows = vec![row(1, 48), row(2, 512), row(3, CHECKPOINT_ITEM_BYTES * 2)];
    let entries = values(&rows);
    let head = PreparedTranscriptHead::new(rows);
    head.capture(&mut Capture::new(1));
    let budgets = (0..2048)
        .chain((CHECKPOINT_ITEM_BYTES - 3)..=(CHECKPOINT_ITEM_BYTES + 3))
        .chain([CHECKPOINT_TARGET_BYTES / 8]);
    for remaining in budgets {
        compare(&head, &entries, remaining, 100);
    }
}

#[test]
fn transcript_row_owners_reindex_deferred_bindings_without_changing_old_cut() {
    let large = row(100, CHECKPOINT_ITEM_BYTES * 3);
    let old = PreparedTranscriptHead::new(vec![large.clone()]);
    let mut capture = Capture::new(7);
    old.capture(&mut capture);
    let old_bytes = serde_json::to_vec(&capture.content).unwrap();
    assert!(
        !capture.content.is_empty(),
        "oversized text exercises positional metadata"
    );
    let mut moved: Vec<_> = (1..=10).map(|cursor| row(cursor, 16)).collect();
    moved.push(large);
    let head = PreparedTranscriptHead::new(moved.clone());
    compare(&head, &values(&moved), CHECKPOINT_TARGET_BYTES / 8, 1000);
    let mut capture = Capture::new(1000);
    head.capture(&mut capture);
    assert!(
        capture
            .content
            .iter()
            .any(|binding| binding.path.get(2).is_some_and(|index| index == "10"))
    );
    let mut capture = Capture::new(7);
    old.capture(&mut capture);
    assert_eq!(serde_json::to_vec(&capture.content).unwrap(), old_bytes);
}

#[test]
fn transcript_row_owners_keep_opaque_numbers_order_and_current_decoration() {
    for argument in [
        serde_json::json!(-0.0),
        serde_json::json!(0.0),
        serde_json::json!({"alpha":1,"beta":2}),
        serde_json::json!({"beta":2,"alpha":1}),
        serde_json::json!(u64::MAX),
    ] {
        let mut value = entry(1, 0);
        let RuntimeClientTranscriptItem::Message {
            message: MessageBlock::Assistant(message),
        } = &mut value.item
        else {
            unreachable!()
        };
        message.content = vec![AssistantContentBlock::ToolCall(
            crate::tools::types::ToolCall {
                id: crate::runtime::identity::ToolCallId::new("call"),
                tool_id: crate::runtime::identity::ToolId::new("tool"),
                name: "inspect".into(),
                arguments: argument,
            },
        )];
        let row = PreparedTranscriptRow::new(conversation(), value.clone());
        let head = PreparedTranscriptHead::new(vec![row.clone()]);
        compare(&head, &[value.clone()], CHECKPOINT_TARGET_BYTES / 8, 1);
        value.response_pending = true;
        let changed = PreparedTranscriptRow::new(conversation(), value.clone());
        assert!(!row.same_owner(&changed));
        compare(
            &PreparedTranscriptHead::new(vec![changed]),
            &[value],
            CHECKPOINT_TARGET_BYTES / 8,
            2,
        );
    }
    let wide = entry(u64::MAX, 4);
    compare(
        &PreparedTranscriptHead::from_entries(&conversation(), std::slice::from_ref(&wide)),
        &[wide],
        CHECKPOINT_TARGET_BYTES / 8,
        1,
    );
    let entries: Vec<_> = (1..=40).map(|cursor| entry(cursor, 4)).collect();
    compare(
        &PreparedTranscriptHead::from_entries(&conversation(), &entries),
        &entries,
        CHECKPOINT_TARGET_BYTES / 8,
        1,
    );
}

#[test]
fn transcript_page_capture_reuses_same_owners_without_row_reindex_or_rebuild() {
    let mut rows = vec![row(1, 64), row(2, CHECKPOINT_ITEM_BYTES * 3)];
    let mut head = PreparedTranscriptHead::new(rows.clone());
    take_work();
    take_page_work();
    let mut first = Capture::new(7);
    let retained = head.capture(&mut first).0.into_owned();
    assert!(!first.content.is_empty());
    assert_eq!(take_work(), 2);
    assert_eq!(take_page_work(), [1, 0, 1, 1]);
    let old = head.clone();

    // A fresh native read may wrap the same proven rows in a new head. Keep
    // the assembled page only for the same total and ordered owner identities.
    head.replace(PreparedTranscriptHead::new(rows.clone()));
    assert!(Arc::ptr_eq(&head.0, &old.0));
    for revision in [8, 99, 1000] {
        compare(&head, &values(&rows), CHECKPOINT_TARGET_BYTES / 8, revision);
    }
    assert_eq!(take_work(), 0);
    assert_eq!(take_page_work(), [0, 0, 3, 0]);

    rows.swap(0, 1);
    head.replace(PreparedTranscriptHead::new(rows.clone()));
    assert!(!head.same_owners(&old));
    compare(&head, &values(&rows), CHECKPOINT_TARGET_BYTES / 8, 1001);
    assert_eq!(take_work(), 0, "moving rows reuse their normalization");
    assert_eq!(take_page_work(), [1, 0, 1, 1]);
    let mut previous = Capture::new(7);
    assert_eq!(old.capture(&mut previous).0.into_owned(), retained);
    assert_eq!(previous.collections, first.collections);
    assert_eq!(
        serde_json::to_vec(&previous.content).unwrap(),
        serde_json::to_vec(&first.content).unwrap()
    );

    // DTO equality is preserved for callers, but never grants cache identity.
    let equal = head.clone();
    rows[0] = PreparedTranscriptRow::new(conversation(), rows[0].value().clone());
    let replacement = PreparedTranscriptHead::new(rows.clone());
    assert_eq!(equal, replacement);
    assert!(!equal.same_owners(&replacement));
    head.replace(replacement);
    take_page_work();
    compare(&head, &values(&rows), CHECKPOINT_TARGET_BYTES / 8, 1002);
    assert_eq!(take_work(), 1);
    assert_eq!(take_page_work(), [1, 0, 1, 1]);
}

#[test]
fn transcript_page_capture_budget_boundary_preserves_oracle_and_warm_certificate() {
    let rows = vec![row(1, 48), row(2, CHECKPOINT_ITEM_BYTES * 3), row(3, 64)];
    let entries = values(&rows);
    let head = PreparedTranscriptHead::new(rows);
    head.capture(&mut Capture::new(1));
    let required = head.0.prepared.get().unwrap().capture.required;
    assert!(required > 0 && required <= CHECKPOINT_TARGET_BYTES / 8);
    take_page_work();
    for remaining in [0, 1, required - 1] {
        compare(&head, &entries, remaining, 100);
    }
    let work = take_page_work();
    assert_eq!(work[..3], [0, 3, 0]);
    take_work();
    for remaining in [required, CHECKPOINT_TARGET_BYTES / 8] {
        compare(&head, &entries, remaining, 101);
    }
    assert_eq!(take_work(), 0);
    assert_eq!(take_page_work(), [0, 0, 2, 0]);

    // Unsafe native integers disable the fit certificate, not byte-exact
    // capture reuse; the caller must still use structural final admission.
    let wide = entry(u64::MAX, 4);
    let head = PreparedTranscriptHead::from_entries(&conversation(), std::slice::from_ref(&wide));
    compare(
        &head,
        std::slice::from_ref(&wide),
        CHECKPOINT_TARGET_BYTES / 8,
        1,
    );
    assert!(head.0.prepared.get().unwrap().fit.is_none());
    take_work();
    take_page_work();
    compare(&head, &[wide], CHECKPOINT_TARGET_BYTES / 8, 2);
    assert_eq!(take_work(), 0);
    assert_eq!(take_page_work(), [0, 0, 1, 0]);
}

#[test]
fn transcript_page_capture_preserves_empty_and_truncated_cardinality() {
    let mut empty = PreparedTranscriptHead::default();
    compare(&empty, &[], CHECKPOINT_TARGET_BYTES / 8, 1);
    let previous = empty.clone();
    empty.replace(PreparedTranscriptHead::new(Vec::new()));
    assert!(Arc::ptr_eq(&empty.0, &previous.0));
    compare(&empty, &[], 0, 2);

    // The generic prepared path shares the same replay rule. Empty admission
    // preserves an earlier domain's requirement instead of adding or clearing it.
    let empty_owner = OnceLock::new();
    for (remaining, required) in [(0, 37), (17, 41), (CHECKPOINT_TARGET_BYTES / 8, 53)] {
        let mut capture = Capture::new(9);
        capture.remaining = remaining;
        capture.required = required;
        capture.evicted = true;
        let (items, _) =
            capture.prepared(&empty_owner, |capture| capture.page::<String>("empty", &[]));
        assert!(items.is_empty());
        assert_eq!(capture.remaining, remaining);
        assert_eq!(capture.required, required);
        assert!(capture.evicted);
        assert_eq!(capture.collections.len(), 1);
        assert_eq!(capture.collections[0].revision, 9);
    }

    let rows: Vec<_> = (1..=40).map(|cursor| row(cursor, 4)).collect();
    let mut head = PreparedTranscriptHead::new(rows[..33].to_vec());
    compare(&head, &values(&rows[..33]), CHECKPOINT_TARGET_BYTES / 8, 3);
    let old = head.clone();
    let next = PreparedTranscriptHead::new(rows.clone());
    assert!(
        head.0
            .rows
            .iter()
            .zip(&next.0.rows)
            .all(|(a, b)| a.same_owner(b))
    );
    assert!(
        !head.same_owners(&next),
        "total changes the public frontier"
    );
    head.replace(next);
    take_work();
    take_page_work();
    compare(&head, &values(&rows), CHECKPOINT_TARGET_BYTES / 8, 4);
    assert_eq!(take_work(), 0);
    assert_eq!(take_page_work(), [1, 0, 1, CHECKPOINT_PAGE_ITEMS - 1]);
    compare(&old, &values(&rows[..33]), CHECKPOINT_TARGET_BYTES / 8, 3);
}
