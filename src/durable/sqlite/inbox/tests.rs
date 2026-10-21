//! Pending inbound inbox regressions of the SQLite store.

use crate::conversation::SurfaceRevision;
use crate::durable::sqlite::test_support::*;
use crate::durable::sqlite::*;
use crate::durable::{CanonicalStore, InboxStore};
use crate::runtime::identity::ConversationId;
use chrono::TimeZone;
use chrono::Utc;

mod pending_transaction;

#[test]
fn acceptance_and_adoption_share_durable_identity() {
    let store = store();
    let accepted = store.accept_inbound(draft("hello")).unwrap();
    let adopted = store.adopt_pending_batch(accepted.sequence, None).unwrap();
    assert_eq!(adopted.items.len(), 1);
    assert!(store.load_pending().unwrap().is_empty());
    assert_eq!(store.load_head().unwrap().active_message_ids.len(), 1);
}

/// The Session lifecycle classification fact: durable acceptance is a
/// monotonic watermark. Adoption moves the accepted work from Pending
/// Inbound into the canonical Ledger and Surface, but never rewinds the
/// acceptance itself — once a conversation has accepted user work, it
/// has accepted user work forever.
#[test]
fn acceptance_watermark_is_monotonic_across_adoption() {
    let store = store();
    assert!(
        !store.has_accepted_inbound().unwrap(),
        "a fresh conversation has never accepted work"
    );

    let accepted = store.accept_inbound(draft("work")).unwrap();
    assert!(
        store.has_accepted_inbound().unwrap(),
        "the acceptance commit advances the watermark"
    );

    let adopted = store.adopt_pending_batch(accepted.sequence, None).unwrap();
    assert_eq!(adopted.items.len(), 1);
    assert!(store.load_pending().unwrap().is_empty());
    assert!(
        store.has_accepted_inbound().unwrap(),
        "adoption cannot rewind the acceptance fact"
    );
}

/// Seeding a lineage writes canonical and Surface history directly; it
/// is not an inbound acceptance and must not raise the watermark. This
/// is what keeps a lineage cut at an empty boundary honest: usage
/// classification for those destinations is the catalog's provenance
/// decision, never a store artifact.
#[test]
fn lineage_seeding_is_not_an_acceptance() {
    let store = store();
    store
        .initialize_lineage(&crate::durable::LineageSeed::history(vec![user_message(
            "seeded-user",
            "seeded history",
        )]))
        .unwrap();
    assert_eq!(store.load_canonical().unwrap().len(), 1);
    assert!(
        !store.has_accepted_inbound().unwrap(),
        "a seeded lineage carries history without any acceptance"
    );
}

#[test]
fn inbound_adoption_fault_keeps_pending_and_surface_atomic() {
    let store = store();
    store.arm_fail_next_accept_commit();
    assert!(store.accept_inbound(draft("not committed")).is_err());
    assert!(store.load_pending().unwrap().is_empty());

    let accepted = store.accept_inbound(draft("pending")).unwrap();
    store.arm_fail_next_adopt_commit();
    assert!(store.adopt_pending_batch(accepted.sequence, None,).is_err());
    assert_eq!(store.load_pending().unwrap().len(), 1);
    assert!(store.load_canonical().unwrap().is_empty());
    assert_eq!(
        store.load_head().unwrap().revision,
        SurfaceRevision::INITIAL
    );

    let adopted = store.adopt_pending_batch(accepted.sequence, None).unwrap();
    assert_eq!(adopted.items.len(), 1);
    assert!(store.load_pending().unwrap().is_empty());
    assert_eq!(store.load_head().unwrap().active_message_ids.len(), 1);
}

#[test]
fn t14_t15_t20_pending_native_receipt_refills_head_without_a_body_mirror() {
    let id = ConversationId::new("conv_2be184c1-ab11-725b-82c3-81daea936186");
    let store = SqliteConversationStore::in_memory(id).unwrap();
    let time = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let mut accepted = Vec::new();
    for index in 0..73 {
        let item = store
            .accept_inbound(crate::durable::inbox::InboundDraft {
                message_id: None,
                source: crate::message::types::UserSource::Human,
                kind: crate::message::types::InboundKind::Message,
                content: vec![crate::message::types::UserContentBlock::Text(
                    crate::message::content::TextBlock {
                        text: format!("pending{index}"),
                    },
                )],
                timestamp: time,
                correlation: None,
            })
            .unwrap();
        let receipt = item
            .pending_observation
            .as_ref()
            .expect("native indexed pending receipt");
        assert_eq!(receipt.total, index + 1);
        assert!(receipt.head.len() <= 32);
        assert_eq!(
            receipt.selected.as_ref().unwrap().sequence,
            item.sequence.get()
        );
        assert!(receipt.head.iter().all(
            |header| header.message.locator.is_none() && header.message.content.is_empty()
        ));
        accepted.push(item);
    }
    let first = &accepted[0];
    let removed = store
        .remove_pending(&crate::durable::inbox::PendingInboundRef {
            sequence: first.sequence,
            revision: 0,
        })
        .unwrap();
    let crate::durable::inbox::PendingMutationOutcome::Applied {
        pending_observation: Some(receipt),
        ..
    } = removed
    else {
        panic!("native pending removal receipt")
    };
    assert_eq!(receipt.total, 72);
    assert_eq!(receipt.removed_sequence, Some(1));
    assert_eq!(receipt.removed_transcript, first.transcript_cursor);
    assert_eq!(
        (
            receipt.head.first().unwrap().sequence,
            receipt.head.last().unwrap().sequence
        ),
        (2, 33)
    );
    assert_eq!(
        accepted
            .last()
            .unwrap()
            .pending_observation
            .as_ref()
            .unwrap()
            .total,
        73,
        "capture evidence remains immutable"
    );
}

/// T14/T20: `GoalRound` accounting, pending head and J frontier are one source receipt.
#[test]
fn t14_t20_goal_receipt_owns_pending_and_journal_atomically() {
    let store = store();
    let (result, receipt) = store
        .write_goal(crate::goal::GoalWrite::Create {
            objective: "goal objective".into(),
            budget: 3,
            origin: crate::goal::GoalOrigin::RuntimeControl,
        })
        .unwrap();
    let goal = result.unwrap();
    let receipt = receipt.unwrap();
    assert_eq!(
        receipt.view.current.as_ref().unwrap().reference,
        goal.reference
    );
    assert!(receipt.pending.is_none());
    assert_eq!(receipt.native.as_ref().unwrap().journal_range, Some((1, 1)));
    let accepted = store
        .accept_goal_round(
            &goal.reference,
            crate::durable::inbox::InboundDraft {
                message_id: None,
                source: crate::message::types::UserSource::Runtime,
                kind: crate::message::types::InboundKind::GoalContinuation(goal.reference.clone()),
                content: vec![crate::message::types::UserContentBlock::Text(
                    crate::message::content::TextBlock {
                        text: "continue".into(),
                    },
                )],
                timestamp: Utc.with_ymd_and_hms(2026, 8, 27, 6, 0, 0).unwrap(),
                correlation: None,
            },
        )
        .unwrap()
        .unwrap();
    let receipt = accepted.goal_observation.as_ref().unwrap();
    let current = receipt.view.current.as_ref().unwrap();
    assert_eq!(current.reference.revision, goal.reference.revision + 1);
    assert_eq!(current.autonomous_rounds_consumed, 1);
    let pending = receipt.pending.as_ref().unwrap();
    assert_eq!(pending.read_cut, accepted.read_cut);
    assert_eq!(pending.total, 1);
    assert_eq!(
        pending.selected.as_ref().unwrap().sequence,
        accepted.sequence.get()
    );
    let native = receipt.native.as_ref().unwrap();
    assert_eq!(native.read_cut.read, accepted.read_cut);
    assert_eq!(native.journal_range, Some((2, 2)));
}
