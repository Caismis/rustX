//! Force both claim/mutation orders at the real file-backed `SQLite` writer.
//! No mailbox exists here. A post-BEGIN probe parks the first transaction;
//! `SQLite`'s busy callback proves the other connection attempted BEGIN and was
//! denied the writer. Channels release the owner, then permit `SQLite` to retry.

use super::*;
use crate::durable::inbox::{PendingInboundRef, PendingMutationOutcome, TranscriptItem};
use chrono::TimeZone;
use std::cell::RefCell;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

#[derive(Debug, PartialEq, Eq)]
enum ContenderProgress {
    Busy,
    Body,
}

struct BusyWait {
    blocked: Sender<ContenderProgress>,
    retry: Receiver<()>,
}

thread_local! {
    // rusqlite accepts a function pointer, not a capturing busy callback.
    // Each contender thread/connection owns its own one-shot handshake.
    static BUSY_WAIT: RefCell<Option<BusyWait>> = const { RefCell::new(None) };
}

fn pending_writer_busy(_previous_calls: i32) -> bool {
    BUSY_WAIT.with(|slot| {
        let Some(wait) = slot.take() else {
            return false; // Unexpected additional contention fails, never polls.
        };
        wait.blocked.send(ContenderProgress::Busy).is_ok() && wait.retry.recv().is_ok()
    })
}

#[derive(Clone, Copy)]
enum Operation {
    Edit,
    Remove,
    Claim,
}

enum Outcome {
    Mutation(PendingMutationOutcome),
    Claimed(Vec<PendingInboundItem>),
}

impl Operation {
    fn probe(self, store: &SqliteConversationStore, probe: impl FnOnce() + Send + 'static) {
        let slot = match self {
            Self::Edit | Self::Remove => &store.pending_mutation_transaction_probe,
            Self::Claim => &store.pending_adoption_transaction_probe,
        };
        *slot.lock().unwrap() = Some(Box::new(probe));
    }

    fn run(
        self,
        store: &SqliteConversationStore,
        expected: &PendingInboundRef,
        watermark: InboundSequence,
    ) -> Outcome {
        match self {
            Self::Edit => Outcome::Mutation(store.edit_pending(expected, "edited").unwrap()),
            Self::Remove => Outcome::Mutation(store.remove_pending(expected).unwrap()),
            Self::Claim => Outcome::Claimed(store.adopt_pending_batch(watermark, None).unwrap()),
        }
    }
}

fn force_writer_order(
    first: Arc<SqliteConversationStore>,
    second: Arc<SqliteConversationStore>,
    first_operation: Operation,
    second_operation: Operation,
    expected: PendingInboundRef,
    watermark: InboundSequence,
) -> (Outcome, Outcome) {
    assert!(!Arc::ptr_eq(&first.conn, &second.conn));
    let (acquired, owner_acquired) = mpsc::channel();
    let (release, owner_release) = mpsc::channel();
    first_operation.probe(&first, move || {
        acquired.send(()).unwrap();
        owner_release.recv().unwrap();
    });
    let first_expected = expected.clone();
    let owner = std::thread::spawn(move || first_operation.run(&first, &first_expected, watermark));
    owner_acquired.recv().unwrap(); // A owns the actual BEGIN IMMEDIATE writer.

    let (blocked, contender_progress) = mpsc::channel();
    let entered = blocked.clone();
    second_operation.probe(&second, move || {
        entered.send(ContenderProgress::Body).unwrap();
    });
    let (retry, contender_retry) = mpsc::channel();
    let contender = std::thread::spawn(move || {
        BUSY_WAIT.set(Some(BusyWait {
            blocked,
            retry: contender_retry,
        }));
        second
            .conn
            .lock()
            .unwrap()
            .busy_handler(Some(pending_writer_busy))
            .unwrap();
        let result = second_operation.run(&second, &expected, watermark);
        second.conn.lock().unwrap().busy_handler(None).unwrap();
        result
    });

    // This signal comes FROM SQLite on B's connection, not from a pre-BEGIN
    // thread/barrier. B attempted the conflicting transaction while A owned it.
    assert_eq!(contender_progress.recv().unwrap(), ContenderProgress::Busy);
    assert_eq!(contender_progress.try_recv(), Err(TryRecvError::Empty));
    release.send(()).unwrap();
    let first_outcome = owner.join().unwrap(); // A committed before B may retry.
    assert_eq!(contender_progress.try_recv(), Err(TryRecvError::Empty));
    retry.send(()).unwrap();
    assert_eq!(contender_progress.recv().unwrap(), ContenderProgress::Body);
    (first_outcome, contender.join().unwrap())
}

fn draft(text: &str) -> InboundDraft {
    InboundDraft {
        message_id: None,
        source: UserSource::Human,
        kind: InboundKind::Message,
        content: vec![UserContentBlock::Text(TextBlock {
            text: text.to_owned(),
        })],
        timestamp: Utc.with_ymd_and_hms(2026, 9, 16, 12, 0, 0).unwrap(),
        correlation: None,
    }
}

fn claim_mutation_race(mutation: Operation, claim_first: bool, retain_another: bool) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pending.db");
    let first =
        Arc::new(SqliteConversationStore::open(ConversationId::new("race"), &path).unwrap());
    let second =
        Arc::new(SqliteConversationStore::open(ConversationId::new("race"), &path).unwrap());
    let accepted = first.accept_inbound(draft("original")).unwrap();
    if retain_another {
        first.accept_inbound(draft("retained")).unwrap();
    }
    // Deliberately retain the pre-mutation selection: only its finite watermark
    // is passed to claim. It must never supply post-commit content or membership.
    let selected = first.select_pending_batch().unwrap().unwrap();
    let expected = PendingInboundRef {
        sequence: accepted.sequence,
        message_id: accepted.message_id.clone(),
        revision: 0,
    };
    let (first_operation, second_operation) = if claim_first {
        (Operation::Claim, mutation)
    } else {
        (mutation, Operation::Claim)
    };
    let (first_outcome, second_outcome) = force_writer_order(
        first.clone(),
        second.clone(),
        first_operation,
        second_operation,
        expected,
        selected.watermark,
    );
    let (claim, mutation_outcome) = if claim_first {
        (first_outcome, second_outcome)
    } else {
        (second_outcome, first_outcome)
    };
    let Outcome::Claimed(receipt) = claim else {
        panic!("claim receipt")
    };
    let Outcome::Mutation(outcome) = mutation_outcome else {
        panic!("mutation outcome")
    };
    assert_eq!(
        outcome,
        if claim_first {
            PendingMutationOutcome::NotPending
        } else {
            PendingMutationOutcome::Applied
        }
    );

    let mut expected_receipt = selected.items.clone();
    if !claim_first {
        match mutation {
            Operation::Edit => {
                expected_receipt[0].message.content = draft("edited").content;
                expected_receipt[0].revision = 1;
            }
            Operation::Remove => {
                expected_receipt.remove(0);
            }
            Operation::Claim => unreachable!(),
        }
    }
    // Full equality covers occurrence identity, order, content, revision, cursor
    // and correlation; the old selected payload cannot leak into the receipt.
    assert_eq!(receipt, expected_receipt);
    assert_claim_read_models(&first, &second, &receipt, &path);
}

fn assert_claim_read_models(
    first: &SqliteConversationStore,
    second: &SqliteConversationStore,
    receipt: &[PendingInboundItem],
    path: &Path,
) {
    let canonical: Vec<_> = receipt
        .iter()
        .map(|item| MessageBlock::User(item.message.clone()))
        .collect();
    let ids: Vec<_> = receipt.iter().map(|item| item.message_id.clone()).collect();
    assert_eq!(first.load_canonical().unwrap(), canonical);
    let head = first.load_head().unwrap();
    assert_eq!(head.active_message_ids, ids);
    assert_eq!(
        first.load_surface_snapshot(head.revision).unwrap(),
        canonical
    );
    let obligations: Vec<_> = first
        .read_events(None, 64)
        .unwrap()
        .events
        .into_iter()
        .map(|event| {
            let RuntimeEvent::InboundTurnAdopted { message_ids } = event.event else {
                panic!("unexpected journal event")
            };
            message_ids
        })
        .collect();
    assert_eq!(obligations, if ids.is_empty() { vec![] } else { vec![ids] });
    assert!(first.load_pending().unwrap().is_empty());
    assert!(second.load_pending().unwrap().is_empty());
    let transcript = first.load_transcript_page(None, 64).unwrap();
    assert_eq!(transcript.entries.len(), receipt.len());
    for (entry, item) in transcript.entries.iter().zip(receipt) {
        assert_eq!(Some(entry.cursor), item.transcript_cursor);
        assert_eq!(
            entry.item,
            TranscriptItem::Message {
                message: MessageBlock::User(item.message.clone())
            }
        );
    }
    // Reopen to assert only committed facts survived; stale remove did not
    // delete canonical transcript state, successful remove left no phantom row.
    let reopened = SqliteConversationStore::open(ConversationId::new("race"), path).unwrap();
    assert_eq!(reopened.load_canonical().unwrap(), canonical);
    assert_eq!(reopened.load_transcript_page(None, 64).unwrap(), transcript);
    assert!(reopened.load_pending().unwrap().is_empty());
}

#[test]
fn sqlite_writer_edit_wins_before_claim() {
    claim_mutation_race(Operation::Edit, false, false);
}

#[test]
fn sqlite_writer_claim_wins_before_edit() {
    claim_mutation_race(Operation::Edit, true, false);
}

#[test]
fn sqlite_writer_remove_wins_before_claim() {
    claim_mutation_race(Operation::Remove, false, false);
}

#[test]
fn sqlite_writer_remove_wins_before_claim_with_retained_occurrence() {
    claim_mutation_race(Operation::Remove, false, true);
}

#[test]
fn sqlite_writer_claim_wins_before_remove() {
    claim_mutation_race(Operation::Remove, true, false);
}
