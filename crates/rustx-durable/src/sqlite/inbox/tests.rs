//! Pending inbound inbox regressions of the SQLite store.

use std::sync::{Arc, Barrier};
use std::thread;

use crate::sqlite::test_support::*;
use crate::sqlite::*;
use crate::{
    CanonicalStore, ConversationStoreError, InboxStore, ObservationStore, PendingInboundRef,
    PendingMutationOutcome, PendingSteerScope,
};
use chrono::TimeZone;
use chrono::Utc;
use rustx_conversation::SurfaceRevision;
use rustx_types::message::types::{InboundKind, MessageBlock, UserSource};
use rustx_types::runtime::identity::{AttemptId, ConversationId};

mod pending_transaction;

#[test]
fn acceptance_and_adoption_share_durable_identity() {
    let store = store();
    let accepted = store.accept_inbound(draft("hello")).unwrap();
    let adopted = store.adopt_pending_batch(accepted.sequence, None).unwrap();
    assert_eq!(adopted.items.len(), 1);
    assert_eq!(
        store.load_pending().unwrap(),
        [] as [crate::inbox::PendingInboundItem; 0]
    );
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
    assert_eq!(
        store.load_pending().unwrap(),
        [] as [crate::inbox::PendingInboundItem; 0]
    );
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
        .initialize_lineage(&crate::LineageSeed::history(vec![user_message(
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
    assert_eq!(
        store.load_pending().unwrap(),
        [] as [crate::inbox::PendingInboundItem; 0]
    );

    let accepted = store.accept_inbound(draft("pending")).unwrap();
    store.arm_fail_next_adopt_commit();
    assert!(store.adopt_pending_batch(accepted.sequence, None,).is_err());
    assert_eq!(store.load_pending().unwrap().len(), 1);
    assert_eq!(
        store.load_canonical().unwrap(),
        [] as [rustx_types::message::types::MessageBlock; 0]
    );
    assert_eq!(
        store.load_head().unwrap().revision,
        SurfaceRevision::INITIAL
    );

    let adopted = store.adopt_pending_batch(accepted.sequence, None).unwrap();
    assert_eq!(adopted.items.len(), 1);
    assert_eq!(
        store.load_pending().unwrap(),
        [] as [crate::inbox::PendingInboundItem; 0]
    );
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
            .accept_inbound(crate::InboundDraft {
                message_id: None,
                source: rustx_types::message::types::UserSource::Human,
                kind: rustx_types::message::types::InboundKind::Message,
                content: vec![rustx_types::message::types::UserContentBlock::Text(
                    rustx_types::message::content::TextBlock {
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
        assert!(receipt
            .head
            .iter()
            .all(|header| header.message.locator.is_none() && header.message.content.is_empty()));
        accepted.push(item);
    }
    let first = &accepted[0];
    let removed = store
        .remove_pending(&crate::PendingInboundRef {
            sequence: first.sequence,
            revision: 0,
        })
        .unwrap();
    let crate::PendingMutationOutcome::Applied {
        pending_observation: Some(receipt),
        ..
    } = removed
    else {
        panic!("native pending removal receipt")
    };
    assert_eq!(receipt.total, 72);
    assert_eq!(receipt.removed_sequence, Some(1));
    assert_eq!(first.transcript_cursor, None);
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
        .write_goal(rustx_types::goal_lifecycle::GoalWrite::Create {
            objective: "goal objective".into(),
            budget: 3,
            origin: rustx_types::goal::GoalOrigin::RuntimeControl,
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
            crate::InboundDraft {
                message_id: None,
                source: rustx_types::message::types::UserSource::Runtime,
                kind: rustx_types::message::types::InboundKind::GoalContinuation(
                    goal.reference.clone(),
                ),
                content: vec![rustx_types::message::types::UserContentBlock::Text(
                    rustx_types::message::content::TextBlock {
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

#[test]
fn active_steering_adopts_exact_rows_and_keeps_interleaved_queue_pending() {
    let store = store();
    let first = store.accept_inbound(draft("steer-before")).unwrap();
    let mut queued = draft("explicit-next-attempt");
    queued.kind = rustx_types::message::types::InboundKind::QueuedMessage;
    let queued = store.accept_inbound(queued).unwrap();
    let last = store.accept_inbound(draft("steer-after")).unwrap();
    let adopted = store
        .adopt_pending_batch(
            last.sequence,
            Some(rustx_types::runtime::identity::AttemptId::new("active")),
        )
        .unwrap();
    assert_eq!(
        adopted
            .items
            .iter()
            .map(|item| item.sequence)
            .collect::<Vec<_>>(),
        vec![first.sequence, last.sequence]
    );
    assert_eq!(
        store
            .load_pending()
            .unwrap()
            .iter()
            .map(|item| item.sequence)
            .collect::<Vec<_>>(),
        vec![queued.sequence]
    );
    let next = store.adopt_pending_batch(queued.sequence, None).unwrap();
    assert_eq!(next.items.len(), 1);
    assert_eq!(next.items[0].sequence, queued.sequence);
    assert_eq!(
        store.load_pending().unwrap(),
        [] as [crate::inbox::PendingInboundItem; 0]
    );
    assert_eq!(store.load_canonical().unwrap().len(), 3);
}

#[test]
fn reopened_chat_and_model_input_share_the_adoption_order() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("adoption.db");
    let conversation = ConversationId::new("conv_129ce50d-d90b-7244-8587-63d3f10932a9");
    let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    let queued = store.accept_inbound(queued("next turn")).unwrap();
    let steer = store.accept_inbound(draft("next step")).unwrap();
    store
        .append_canonical(&assistant_message("before-adoption", "original reply"))
        .unwrap();
    assert_eq!(
        store.load_transcript_page(None, 8).unwrap().entries.len(),
        1
    );
    // Reopening before the claim must preserve the steering and queue lanes.
    drop(store);
    let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    store.adopt_pending_batch(steer.sequence, None).unwrap();
    drop(store);
    let store = SqliteConversationStore::open(conversation, &path).unwrap();
    let ids = message_ids(&store);
    assert_eq!(
        ids,
        [
            "before-adoption",
            steer.message_id.as_str(),
            queued.message_id.as_str()
        ]
    );
    let transcript = store.load_transcript_page(None, 8).unwrap();
    assert_eq!(transcript.entries.len(), ids.len());
    for (entry, id) in transcript.entries.iter().zip(ids) {
        let crate::TranscriptItem::Message { message } = &entry.item else {
            panic!("expected adopted chat message");
        };
        assert_eq!(message.id().as_str(), id);
    }
}

fn queued(text: &str) -> crate::InboundDraft {
    let mut draft = draft(text);
    draft.kind = InboundKind::QueuedMessage;
    draft
}

fn expected(sequence: crate::inbound::InboundSequence, revision: u64) -> PendingInboundRef {
    PendingInboundRef { sequence, revision }
}

fn steer_row(
    store: &SqliteConversationStore,
    sequence: crate::inbound::InboundSequence,
    revision: u64,
) -> PendingMutationOutcome {
    store
        .steer_pending(&PendingSteerScope::Row {
            expected: expected(sequence, revision),
        })
        .unwrap()
}

fn message_ids(store: &SqliteConversationStore) -> Vec<String> {
    store
        .load_canonical()
        .unwrap()
        .iter()
        .map(|message| message.id().as_str().to_owned())
        .collect()
}

#[test]
fn steer_all_respects_the_observed_queue_boundary_after_a_text_edit() {
    let store = store();
    let mut first = queued("first");
    first.content.extend(draft(" part").content);
    let first = store.accept_inbound(first).unwrap();
    let later = store.accept_inbound(queued("later")).unwrap();
    assert!(matches!(
        store
            .edit_pending(&expected(first.sequence, 0), "edited")
            .unwrap(),
        PendingMutationOutcome::Applied { .. }
    ));
    store
        .steer_pending(&PendingSteerScope::All {
            through: first.sequence,
        })
        .unwrap();
    let rows = store.load_pending().unwrap();
    assert_eq!(rows[0].message.kind, InboundKind::Message);
    assert_eq!(rows[0].message.content, draft("edited").content);
    assert_eq!(rows[1].message_id, later.message_id);
    assert_eq!(rows[1].message.kind, InboundKind::QueuedMessage);
}

#[test]
fn steering_a_queued_row_keeps_identity_sequence_and_order() {
    let store = store();
    let before = store.accept_inbound(draft("steer-before")).unwrap();
    let queued = store.accept_inbound(queued("promote-me")).unwrap();
    let after = store.accept_inbound(draft("steer-after")).unwrap();
    let outcome = steer_row(&store, queued.sequence, 0);
    assert!(matches!(outcome, PendingMutationOutcome::Applied { .. }));
    let pending = store.load_pending().unwrap();
    assert_eq!(pending.len(), 3);
    assert_eq!(pending[0].sequence, before.sequence);
    assert_eq!(pending[0].message.kind, InboundKind::Message);
    assert_eq!(pending[1].message_id, queued.message_id);
    assert_eq!(pending[1].sequence, queued.sequence);
    assert_eq!(pending[1].revision, 1);
    assert_eq!(pending[1].message.kind, InboundKind::Message);
    assert_eq!(pending[1].message.id, queued.message_id);
    assert_eq!(pending[2].sequence, after.sequence);
    let PendingMutationOutcome::Applied {
        pending_observation,
        ..
    } = outcome
    else {
        unreachable!("applied")
    };
    let pending_observation = pending_observation.unwrap();
    let header = pending_observation
        .head
        .iter()
        .find(|header| header.sequence == queued.sequence.get())
        .unwrap();
    let MessageBlock::User(displayed) = header.message.display.as_ref().unwrap() else {
        panic!("steered row must stay a user message")
    };
    assert_eq!(displayed.kind, InboundKind::Message);
    assert_eq!(displayed.id, queued.message_id);
}

#[test]
fn steer_all_promotes_only_eligible_queued_rows_in_sequence_order() {
    let store = store();
    let first = store.accept_inbound(queued("first")).unwrap();
    let steering = store.accept_inbound(draft("already-steering")).unwrap();
    let mut runtime = queued("runtime-owned");
    runtime.source = UserSource::Runtime;
    let runtime = store.accept_inbound(runtime).unwrap();
    let mut foreign = queued("foreign-correlation");
    foreign.correlation = Some("other:1".into());
    let foreign = store.accept_inbound(foreign).unwrap();
    let mut correlated = queued("app-server-row");
    correlated.correlation = Some("app-server:input-1".into());
    let correlated = store.accept_inbound(correlated).unwrap();
    let outcome = store
        .steer_pending(&PendingSteerScope::All {
            through: correlated.sequence,
        })
        .unwrap();
    assert!(matches!(outcome, PendingMutationOutcome::Applied { .. }));
    let pending = store.load_pending().unwrap();
    assert_eq!(
        pending
            .iter()
            .map(|item| (
                item.sequence,
                item.message_id.clone(),
                item.message.kind.clone(),
                item.revision,
            ))
            .collect::<Vec<_>>(),
        vec![
            (
                first.sequence,
                first.message_id.clone(),
                InboundKind::Message,
                1
            ),
            (
                steering.sequence,
                steering.message_id.clone(),
                InboundKind::Message,
                0,
            ),
            (
                runtime.sequence,
                runtime.message_id.clone(),
                InboundKind::QueuedMessage,
                0,
            ),
            (
                foreign.sequence,
                foreign.message_id.clone(),
                InboundKind::QueuedMessage,
                0,
            ),
            (
                correlated.sequence,
                correlated.message_id.clone(),
                InboundKind::Message,
                1,
            ),
        ]
    );
    let again = store
        .steer_pending(&PendingSteerScope::All {
            through: correlated.sequence,
        })
        .unwrap();
    assert!(matches!(again, PendingMutationOutcome::Applied { .. }));
    assert_eq!(store.load_pending().unwrap()[0].revision, 1);
    let adopted = store
        .adopt_pending_batch(correlated.sequence, Some(AttemptId::new("active")))
        .unwrap();
    assert_eq!(
        adopted
            .items
            .iter()
            .map(|item| item.message_id.clone())
            .collect::<Vec<_>>(),
        vec![steering.message_id, first.message_id, correlated.message_id]
    );
    assert_eq!(message_ids(&store).len(), 3);
    assert_eq!(
        store
            .load_pending()
            .unwrap()
            .iter()
            .map(|item| item.message_id.clone())
            .collect::<Vec<_>>(),
        vec![runtime.message_id, foreign.message_id]
    );
}

#[test]
fn row_steer_retries_and_conflicts_do_not_insert_a_second_occurrence() {
    let store = store();
    let queued = store.accept_inbound(queued("once")).unwrap();
    assert!(matches!(
        steer_row(&store, queued.sequence, 7),
        PendingMutationOutcome::Conflict
    ));
    assert_eq!(store.load_pending().unwrap()[0].revision, 0);
    assert!(matches!(
        steer_row(&store, queued.sequence, 0),
        PendingMutationOutcome::Applied { .. }
    ));
    assert!(matches!(
        steer_row(&store, queued.sequence, 0),
        PendingMutationOutcome::Conflict
    ));
    assert!(matches!(
        steer_row(&store, queued.sequence, 1),
        PendingMutationOutcome::InvalidItem
    ));
    assert!(matches!(
        steer_row(&store, crate::inbound::InboundSequence::new(99), 0,),
        PendingMutationOutcome::NotPending
    ));
    assert_eq!(store.load_pending().unwrap().len(), 1);
    let adopted = store
        .adopt_pending_batch(queued.sequence, Some(AttemptId::new("active")))
        .unwrap();
    assert_eq!(adopted.items.len(), 1);
    assert_eq!(adopted.items[0].message_id, queued.message_id);
    assert!(matches!(
        steer_row(&store, queued.sequence, 1),
        PendingMutationOutcome::NotPending
    ));
    assert_eq!(
        message_ids(&store),
        vec![queued.message_id.as_str().to_owned()]
    );
    let again = store
        .adopt_pending_batch(queued.sequence, Some(AttemptId::new("active")))
        .unwrap();
    assert_eq!(again.items, [] as [crate::PendingInboundItem; 0]);
    assert_eq!(message_ids(&store).len(), 1);
}

#[test]
fn steer_races_adoption_and_retries_without_double_delivery() {
    let store = Arc::new(store());
    let first = store.accept_inbound(queued("race")).unwrap();
    let id = first.message_id;
    let sequence = first.sequence;
    let started = Arc::new(Barrier::new(3));
    let steer = {
        let store = Arc::clone(&store);
        let started = Arc::clone(&started);
        thread::spawn(move || {
            started.wait();
            store.steer_pending(&PendingSteerScope::Row {
                expected: expected(sequence, 0),
            })
        })
    };
    let adopt = {
        let store = Arc::clone(&store);
        let started = Arc::clone(&started);
        thread::spawn(move || {
            started.wait();
            store.adopt_pending_batch(sequence, Some(AttemptId::new("active")))
        })
    };
    started.wait();
    steer.join().unwrap().unwrap();
    adopt.join().unwrap().unwrap();
    if !store.load_pending().unwrap().is_empty() {
        store
            .adopt_pending_batch(sequence, Some(AttemptId::new("active")))
            .unwrap();
    }
    assert_eq!(message_ids(&store), vec![id.as_str().to_owned()]);
    assert!(matches!(
        store
            .steer_pending(&PendingSteerScope::All { through: sequence })
            .unwrap(),
        PendingMutationOutcome::Applied { .. }
    ));
    assert_eq!(
        store.load_pending().unwrap(),
        [] as [crate::PendingInboundItem; 0]
    );
    assert_eq!(message_ids(&store).len(), 1);

    let queued = store.accept_inbound(queued("full-adopt-race")).unwrap();
    let id = queued.message_id.clone();
    let sequence = queued.sequence;
    let started = Arc::new(Barrier::new(3));
    let steer = {
        let store = Arc::clone(&store);
        let started = Arc::clone(&started);
        thread::spawn(move || {
            started.wait();
            store.steer_pending(&PendingSteerScope::All { through: sequence })
        })
    };
    let adopt = {
        let store = Arc::clone(&store);
        let started = Arc::clone(&started);
        thread::spawn(move || {
            started.wait();
            store.adopt_pending_batch(sequence, None)
        })
    };
    started.wait();
    steer.join().unwrap().unwrap();
    adopt.join().unwrap().unwrap();
    if !store.load_pending().unwrap().is_empty() {
        store.adopt_pending_batch(sequence, None).unwrap();
    }
    let ids = message_ids(&store);
    assert_eq!(
        ids.iter()
            .filter(|item| item.as_str() == id.as_str())
            .count(),
        1
    );
}

#[test]
#[allow(clippy::too_many_lines)] // Observe the same row from acceptance through fault and adoption.
fn steer_derived_header_fail_preserves_native_cas_and_once_only_adoption() {
    let cases = [
        (
            "transcript_header_fail",
            "CREATE TEMP TRIGGER fail_steer_derived_header \
             BEFORE UPDATE OF header_json ON transcript_entries \
             BEGIN SELECT RAISE(FAIL, 'optional transcript header'); END;",
            true,
        ),
        (
            "pending_header_non_null_fail",
            "CREATE TEMP TRIGGER fail_steer_derived_header \
             BEFORE UPDATE OF observation_header ON pending_inbound \
             WHEN NEW.observation_header IS NOT NULL \
             BEGIN SELECT RAISE(FAIL, 'optional pending header'); END;",
            true,
        ),
        (
            "pending_header_unconditional_fail",
            "CREATE TEMP TRIGGER fail_steer_derived_header \
             BEFORE UPDATE OF observation_header ON pending_inbound \
             BEGIN SELECT RAISE(FAIL, 'optional pending header'); END;",
            false,
        ),
    ];

    for (label, trigger_sql, expect_null_header) in cases {
        for steer_all in [false, true] {
            let store = store();
            let accepted = store.accept_inbound(queued("promote-under-fault")).unwrap();
            let before_cut = accepted.read_cut.clone();
            let initial_page = store.pending_header_page(&before_cut, None, 32).unwrap();
            assert_eq!(initial_page.headers.len(), 1, "{label}");
            let MessageBlock::User(initial_display) =
                initial_page.headers[0].message.display.as_ref().unwrap()
            else {
                panic!("{label}: expected user pending header");
            };
            assert_eq!(initial_display.kind, InboundKind::QueuedMessage, "{label}");
            assert!(
                store
                    .pending_value_read(&before_cut, accepted.sequence.get(), 0, 65_536)
                    .is_ok(),
                "{label}"
            );

            store.lock().unwrap().execute_batch(trigger_sql).unwrap();

            let scope = if steer_all {
                PendingSteerScope::All {
                    through: accepted.sequence,
                }
            } else {
                PendingSteerScope::Row {
                    expected: expected(accepted.sequence, 0),
                }
            };
            let outcome = store.steer_pending(&scope).unwrap();
            let PendingMutationOutcome::Applied {
                read_cut,
                pending_observation,
            } = outcome
            else {
                panic!("{label}: expected applied steer despite derived header RAISE(FAIL)");
            };
            assert!(
                pending_observation.is_none(),
                "{label}: receipt must mark unavailable when derived header fails"
            );
            assert_eq!(
                read_cut.mutation_revision,
                before_cut.mutation_revision + 1,
                "{label}"
            );

            let pending = store.load_pending().unwrap();
            assert_eq!(pending.len(), 1, "{label}");
            assert_eq!(pending[0].sequence, accepted.sequence, "{label}");
            assert_eq!(pending[0].message_id, accepted.message_id, "{label}");
            assert_eq!(pending[0].revision, 1, "{label}");
            assert_eq!(pending[0].message.kind, InboundKind::Message, "{label}");

            let header_is_null: bool = store
                .lock()
                .unwrap()
                .query_row(
                    "SELECT observation_header IS NULL FROM pending_inbound WHERE sequence=?1",
                    [accepted.sequence.get()],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(header_is_null, expect_null_header, "{label}");

            // Stale QueuedMessage header must never be served at the steered cut.
            assert!(
                matches!(
                    store.pending_header_page(&read_cut, None, 32),
                    Err(ConversationStoreError::Storage(_))
                ),
                "{label}: pending header page must fail unavailable instead of returning stale header"
            );
            assert!(
                matches!(
                    store.pending_value_read(&read_cut, accepted.sequence.get(), 0, 65_536),
                    Err(ConversationStoreError::InvalidReference(_))
                ),
                "{label}: stale revision-0 content frontier must be fenced"
            );

            // Dropping the trigger cannot make a stale revision-0 header readable.
            store
                .lock()
                .unwrap()
                .execute_batch("DROP TRIGGER fail_steer_derived_header")
                .unwrap();
            assert!(
                matches!(
                    store.pending_header_page(&read_cut, None, 32),
                    Err(ConversationStoreError::Storage(_))
                ),
                "{label}: stale header must stay unavailable after trigger removal"
            );

            // Authoritative CAS/revision remains intact after faulted steer.
            assert_eq!(
                steer_row(&store, accepted.sequence, 0),
                PendingMutationOutcome::Conflict,
                "{label}"
            );
            assert_eq!(
                steer_row(&store, accepted.sequence, 1),
                PendingMutationOutcome::InvalidItem,
                "{label}"
            );

            // Active-attempt adoption claims the steered row exactly once.
            let adopted = store
                .adopt_pending_batch(accepted.sequence, Some(AttemptId::new("active")))
                .unwrap();
            assert_eq!(adopted.items.len(), 1, "{label}");
            assert_eq!(adopted.items[0].message_id, accepted.message_id, "{label}");
            assert_eq!(adopted.items[0].revision, 1, "{label}");
            assert_eq!(
                adopted.items[0].message.kind,
                InboundKind::Message,
                "{label}"
            );
            assert_eq!(
                store.load_pending().unwrap(),
                [] as [crate::inbox::PendingInboundItem; 0],
                "{label}"
            );
            assert_eq!(
                message_ids(&store),
                vec![accepted.message_id.as_str().to_owned()],
                "{label}"
            );
            let second = store
                .adopt_pending_batch(accepted.sequence, Some(AttemptId::new("active")))
                .unwrap();
            assert_eq!(
                second.items,
                [] as [crate::inbox::PendingInboundItem; 0],
                "{label}"
            );
            assert_eq!(message_ids(&store).len(), 1, "{label}");
        }
    }
}

#[test]
#[allow(clippy::too_many_lines)] // Compare all transaction failure modes at both display writes.
fn steer_propagates_cancellation_and_transaction_invalidity_from_derived_headers() {
    use rusqlite::functions::FunctionFlags;
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization, TransactionOperation};

    let targets = [
        ("transcript_header", "header_json ON transcript_entries"),
        ("pending_header", "observation_header ON pending_inbound"),
    ];

    for (label, target) in targets {
        // 1. Cancellation inside either derived header path propagates OperationCancelled.
        {
            let store = store();
            let accepted = store.accept_inbound(queued("cancel-me")).unwrap();
            let before_cut = accepted.read_cut.clone();
            {
                let connection = store.lock().unwrap();
                let interrupt = connection.get_interrupt_handle();
                connection
                    .create_scalar_function(
                        "test_cancel_steer",
                        0,
                        FunctionFlags::SQLITE_UTF8,
                        move |_| {
                            interrupt.interrupt();
                            Ok(1_i64)
                        },
                    )
                    .unwrap();
                connection
                    .execute_batch(&format!(
                        "CREATE TEMP TRIGGER cancel_steer_header \
                         BEFORE UPDATE OF {target} \
                         BEGIN \
                             SELECT test_cancel_steer(); \
                             SELECT count(*) FROM (WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM r WHERE n<16) SELECT n FROM r); \
                         END;"
                    ))
                    .unwrap();
            }
            let result = store.steer_pending(&PendingSteerScope::Row {
                expected: expected(accepted.sequence, 0),
            });
            assert_eq!(
                result,
                Err(ConversationStoreError::OperationCancelled),
                "{label}: cancellation must propagate"
            );
            let pending = store.load_pending().unwrap();
            assert_eq!(pending.len(), 1, "{label}");
            assert_eq!(pending[0].revision, 0, "{label}");
            assert_eq!(
                pending[0].message.kind,
                InboundKind::QueuedMessage,
                "{label}"
            );
            let page = store.pending_header_page(&before_cut, None, 32).unwrap();
            assert_eq!(page.headers[0].revision, 0, "{label}");
        }

        // 2. Outer transaction abort (RAISE(ROLLBACK)) propagates without degrading.
        {
            let store = store();
            let accepted = store.accept_inbound(queued("rollback-me")).unwrap();
            store
                .lock()
                .unwrap()
                .execute_batch(&format!(
                    "CREATE TEMP TRIGGER rollback_steer_header \
                     BEFORE UPDATE OF {target} \
                     BEGIN SELECT RAISE(ROLLBACK, 'outer tx aborted'); END;"
                ))
                .unwrap();
            let result = store.steer_pending(&PendingSteerScope::Row {
                expected: expected(accepted.sequence, 0),
            });
            assert!(
                matches!(result, Err(ConversationStoreError::Storage(_))),
                "{label}: outer transaction abort must propagate, got {result:?}"
            );
            let pending = store.load_pending().unwrap();
            assert_eq!(pending.len(), 1, "{label}");
            assert_eq!(pending[0].revision, 0, "{label}");
            assert_eq!(
                pending[0].message.kind,
                InboundKind::QueuedMessage,
                "{label}"
            );
        }

        // 3. Savepoint cleanup failure propagates TransactionInvalid.
        {
            let store = store();
            let accepted = store.accept_inbound(queued("invalid-tx")).unwrap();
            {
                let connection = store.lock().unwrap();
                connection
                    .execute_batch(&format!(
                        "CREATE TEMP TRIGGER fail_steer_header \
                         BEFORE UPDATE OF {target} \
                         BEGIN SELECT RAISE(FAIL, 'optional header'); END;"
                    ))
                    .unwrap();
                connection
                    .authorizer(Some(|context: AuthContext<'_>| {
                        if matches!(
                            context.action,
                            AuthAction::Savepoint {
                                operation: TransactionOperation::Rollback,
                                ..
                            }
                        ) {
                            return Authorization::Deny;
                        }
                        Authorization::Allow
                    }))
                    .unwrap();
            }
            let result = store.steer_pending(&PendingSteerScope::Row {
                expected: expected(accepted.sequence, 0),
            });
            store
                .lock()
                .unwrap()
                .authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
                .unwrap();
            assert!(
                matches!(result, Err(ConversationStoreError::TransactionInvalid(_))),
                "{label}: savepoint cleanup failure must propagate TransactionInvalid, got {result:?}"
            );
            let pending = store.load_pending().unwrap();
            assert_eq!(pending.len(), 1, "{label}");
            assert_eq!(pending[0].revision, 0, "{label}");
            assert_eq!(
                pending[0].message.kind,
                InboundKind::QueuedMessage,
                "{label}"
            );
        }
    }
}
