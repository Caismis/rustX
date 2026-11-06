//! Capacity regressions of bounded observation readiness.

use crate::durable::sqlite::test_support::*;
use crate::durable::sqlite::*;
use crate::durable::{CanonicalStore, JournalStore, ObservationStore};
use crate::events::types::RuntimeEvent;
use crate::runtime::identity::{AttemptId, ConversationId};
use std::sync::{Arc, Mutex};

#[test]
fn t14_t19_t20_transcript_source_prefix_recovers_after_4097_later_owner_releases() {
    struct Observer {
        store: std::sync::Weak<SqliteConversationStore>,
        latest: Mutex<Option<crate::durable::observation::TranscriptSourceReady>>,
        wakes: std::sync::atomic::AtomicUsize,
    }
    impl crate::durable::presentation::JournalObserver for Observer {
        fn committed(&self, _through: Option<u64>) {}
        fn ready(&self, through: u64) {
            assert_eq!(
                through, 0,
                "T publication wakes even when J does not change"
            );
            let proof = self
                .store
                .upgrade()
                .unwrap()
                .transcript_source_ready()
                .unwrap();
            *self.latest.lock().unwrap() = Some(proof);
            self.wakes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    let id = ConversationId::new("conv_2be184c1-ab11-725b-82c3-81daea936182");
    let store = Arc::new(SqliteConversationStore::in_memory(id.clone()).unwrap());
    store.initialize(&[]).unwrap();
    let observer = Arc::new(Observer {
        store: Arc::downgrade(&store),
        latest: Mutex::new(None),
        wakes: std::sync::atomic::AtomicUsize::new(0),
    });
    store.observe_journal(observer.clone()).unwrap();
    let attempt = AttemptId::new("held-terminal-t");
    store
        .append_event(envelope(
            &id,
            "held-start",
            Some(attempt.clone()),
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt.clone(),
            },
        ))
        .unwrap();
    store
        .append_event(envelope(
            &id,
            "held-terminal",
            Some(attempt.clone()),
            RuntimeEvent::AttemptTimedOut {
                attempt_id: attempt,
            },
        ))
        .unwrap();
    let messages = (0..4097)
        .map(|index| user_message(&format!("later-t-{index}"), "canonical"))
        .collect::<Vec<_>>();
    let receipts = store.append_canonical_batch(&messages).unwrap();
    let batch = &receipts.last().unwrap().observation;
    assert_eq!(batch.ready_t_through, Some(0));
    assert_eq!(batch.last_transcript.unwrap().get(), 4098);
    let frozen = store.transcript_source_ready().unwrap();
    assert_eq!(frozen.prefix, 0);
    assert_eq!(frozen.read_cut.ledger_through, 4097);
    for position in 2..=4098 {
        store.mark_observation_transcript_ready(position).unwrap();
    }
    let held = observer.latest.lock().unwrap().clone().unwrap();
    assert_eq!(
        held, frozen,
        "later owner releases cannot skip held native T1"
    );
    store.mark_observation_transcript_ready(1).unwrap();
    let released = observer.latest.lock().unwrap().clone().unwrap();
    assert_eq!(released.prefix, 4098);
    assert_eq!(released.read_cut, frozen.read_cut);
    assert_eq!(
        observer.wakes.load(std::sync::atomic::Ordering::SeqCst),
        4098
    );
    assert_eq!(
        batch.ready_t_through,
        Some(0),
        "receipt proof remains immutable"
    );
    let follow = store
        .append_canonical(&user_message("follow-t", "next"))
        .unwrap();
    assert_eq!(follow.observation.ready_t_through, Some(4098));
    let current = store.transcript_source_ready().unwrap();
    assert_eq!(current.prefix, 4098);
    assert_eq!(current.read_cut.read.transcript, 4099);
    assert_eq!(frozen.read_cut.read.transcript, 4098);
}
