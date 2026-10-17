//! Test-only native receipt notices; original events are read outside callbacks.
use crate::durable::{
    ConversationStore,
    inbox::{GoalObservationReceipt, PendingObservationReceipt, PublicationAuditReceipt},
    observation::NativeObservationBatch,
    reading::ConversationReadCut,
};
use crate::events::types::RuntimeEventEnvelope;
use crate::runtime::{
    conversation_runtime::ConversationRuntime,
    identity::{AttemptId, ManualCompactionRequestId},
    interaction::InteractionRef,
    observation::{
        ConversationObservation, NativeReceiptInstallation, PendingObservations,
        PublicationProjectionCommit,
    },
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;
pub(crate) struct NativeJournalRecorder {
    source: std::sync::Weak<dyn ConversationStore>,
    queue: std::sync::Weak<PendingObservations>,
    journal: watch::Sender<u64>,
    pending: watch::Sender<Vec<InteractionRef>>,
    heads: Mutex<BTreeMap<&'static str, Vec<InteractionRef>>>,
}
impl NativeJournalRecorder {
    pub(crate) fn bind(
        runtime: &ConversationRuntime,
        store: &Arc<dyn ConversationStore>,
        queue: &Arc<PendingObservations>,
        through: u64,
    ) -> Arc<Self> {
        let (journal, _) = watch::channel(through);
        let (pending, _) = watch::channel(Vec::new());
        let recorder = Arc::new(Self {
            source: Arc::downgrade(store),
            queue: Arc::downgrade(queue),
            journal,
            pending,
            heads: Mutex::new(BTreeMap::new()),
        });
        runtime.install_publication_projection(recorder.clone());
        recorder
    }
    pub(crate) fn admitted_through(&self) -> u64 {
        *self.journal.borrow()
    }
    pub(crate) fn notice(&self) -> watch::Receiver<u64> {
        self.journal.subscribe()
    }
    pub(crate) fn read_after(&self, after: u64) -> Vec<RuntimeEventEnvelope> {
        let through = self.admitted_through();
        if after >= through {
            return Vec::new();
        }
        let store = self
            .source
            .upgrade()
            .expect("fixture original Journal owner remains alive");
        let events = store
            .read_events(Some(after), 32)
            .expect("fixture original Journal page")
            .events
            .into_iter()
            .take_while(|event| event.sequence <= through)
            .collect::<Vec<_>>();
        assert!(
            !events.is_empty(),
            "committed fixture Journal prefix must exist"
        );
        events
    }
    fn through(&self, through: u64) {
        let changed = self.journal.send_if_modified(|current| {
            if through <= *current {
                return false;
            }
            *current = through;
            true
        });
        if changed && let Some(queue) = self.queue.upgrade() {
            crate::durable::presentation::JournalObserver::committed(&*queue, None);
        }
    }
    fn batch(&self, batch: Option<&NativeObservationBatch>) -> NativeReceiptInstallation {
        if let Some(batch) = batch {
            self.through(batch.read_cut.read.journal);
        }
        NativeReceiptInstallation::Unavailable
    }
    fn journal_observation(&self, observation: &ConversationObservation) {
        match observation {
            ConversationObservation::Published {
                journal_sequence, ..
            } => self.through(*journal_sequence),
            ConversationObservation::CanonicalCommitted { receipt, .. } => {
                self.through(receipt.observation.read_cut.read.journal);
            }
            ConversationObservation::AuditSettled { receipt, .. } => {
                self.through(receipt.read_cut.read.journal);
            }
            ConversationObservation::InteractionPending {
                audit: Some((event, _)),
                ..
            }
            | ConversationObservation::InteractionSettled {
                audit: Some((event, _)),
                ..
            } => self.through(event.sequence),
            ConversationObservation::ObservationBatch(items)
            | ConversationObservation::JournalBatch {
                observations: items,
                ..
            } => {
                for item in items {
                    self.journal_observation(item);
                }
            }
            ConversationObservation::CollectionProof { observation, .. }
            | ConversationObservation::ToolOccurrenceProof { observation, .. } => {
                self.journal_observation(observation);
            }
            _ => {}
        }
    }
    fn observation(&self, observation: &ConversationObservation) {
        self.journal_observation(observation);
        for fact in observation.facts() {
            if let ConversationObservation::PendingHead { owner, head, .. } = fact {
                let mut heads = self.heads.lock().expect("fixture pending heads");
                heads.insert(
                    owner,
                    head.iter()
                        .map(|header| header.interaction.clone())
                        .collect(),
                );
                self.pending
                    .send_replace(heads.values().flatten().cloned().collect());
            }
        }
    }
}
impl PublicationProjectionCommit for NativeJournalRecorder {
    fn native_actor(
        &self,
        observation: &ConversationObservation,
    ) -> Option<NativeReceiptInstallation> {
        self.observation(observation);
        None
    }
    fn native_owner(
        &self,
        observation: &ConversationObservation,
        batch: Option<&NativeObservationBatch>,
    ) -> NativeReceiptInstallation {
        self.observation(observation);
        if let Some(queue) = self.queue.upgrade() {
            queue.push(observation.clone());
        }
        self.batch(batch)
    }
    fn native_batch(&self, batch: &NativeObservationBatch) -> NativeReceiptInstallation {
        self.batch(Some(batch))
    }
    fn native_manual_compaction(
        &self,
        _request: Option<&ManualCompactionRequestId>,
        batch: Option<&NativeObservationBatch>,
    ) -> NativeReceiptInstallation {
        self.batch(batch)
    }
    fn native_goal(&self, receipt: &GoalObservationReceipt) -> NativeReceiptInstallation {
        self.batch(receipt.native.as_deref())
    }
    fn native_audit(&self, _attempt: &AttemptId, receipt: &PublicationAuditReceipt) {
        self.through(receipt.read_cut.read.journal);
    }
    fn native_pending(
        &self,
        receipt: Option<&PendingObservationReceipt>,
        cut: &ConversationReadCut,
    ) {
        if let Some(receipt) = receipt
            && let Some(queue) = self.queue.upgrade()
        {
            queue.push(ConversationObservation::PendingReceipt(Arc::new(
                receipt.clone(),
            )));
        }
        self.through(cut.journal);
    }
    fn opened(&self, _attempt: &AttemptId, _start: &crate::publication::PublicationStreamStart) {}
    fn frame(
        &self,
        _attempt: &AttemptId,
        _frame: &crate::publication::PublicationFrame,
        _receipt: &crate::publication::PublicationFrameReceipt,
    ) {
    }
}

/// One scalar notice and an original Journal cursor; never a client projection ACK.
pub(crate) struct OriginalJournal {
    recorder: Arc<NativeJournalRecorder>,
    journal: watch::Receiver<u64>,
    pending: watch::Receiver<Vec<InteractionRef>>,
    after: u64,
}
impl OriginalJournal {
    pub(crate) fn install(
        runtime: &ConversationRuntime,
        store: &Arc<dyn ConversationStore>,
        queue: &Arc<PendingObservations>,
    ) -> Self {
        let mut after = 0;
        loop {
            let page = store
                .read_events(Some(after), 32)
                .expect("fixture original Journal seed");
            let Some(last) = page.events.last() else {
                break;
            };
            after = last.sequence;
        }
        let recorder = NativeJournalRecorder::bind(runtime, store, queue, after);
        let journal = recorder.notice();
        let pending = recorder.pending.subscribe();
        Self {
            recorder,
            journal,
            pending,
            after,
        }
    }
    pub(crate) fn pending(&self) -> watch::Receiver<Vec<InteractionRef>> {
        self.pending.clone()
    }
    pub(crate) async fn next_page(&mut self) -> Vec<RuntimeEventEnvelope> {
        loop {
            let through = *self.journal.borrow_and_update();
            if self.after < through {
                let events = self.recorder.read_after(self.after);
                self.after = events
                    .last()
                    .expect("committed fixture Journal prefix must exist")
                    .sequence;
                return events;
            }
            self.journal
                .changed()
                .await
                .expect("fixture native receipt owner remains alive");
        }
    }
    pub(crate) async fn wait_for(
        &mut self,
        matches: impl Fn(&RuntimeEventEnvelope) -> bool,
    ) -> RuntimeEventEnvelope {
        loop {
            if let Some(event) = self.next_page().await.into_iter().find(&matches) {
                // Later original rows in this same bounded page remain unread.
                self.after = event.sequence;
                return event;
            }
        }
    }
}
