use crate::durable::observation::NativeObservationBatch;
use crate::runtime::observation::{
    NativeReceiptInstallation, ObservationFanout, PublicationProjectionCommit,
};
use std::sync::atomic::{AtomicBool, Ordering};

struct BatchProjection {
    store: Arc<dyn ConversationStore>,
    installed: AtomicBool,
    accept: bool,
}
impl PublicationProjectionCommit for BatchProjection {
    fn native_batch(&self, batch: &NativeObservationBatch) -> NativeReceiptInstallation {
        let (_, last) = batch.journal_range.unwrap();
        assert_eq!(
            self.store.observation_ready_frontier(0, last).unwrap(),
            0,
            "the native source must remain fenced until CPU installation finishes"
        );
        self.installed.store(self.accept, Ordering::Release);
        if self.accept {
            NativeReceiptInstallation::Installed
        } else {
            NativeReceiptInstallation::Unavailable
        }
    }
    fn native_actor(&self, _: &ConversationObservation) -> Option<NativeReceiptInstallation> {
        unreachable!()
    }
    fn native_owner(
        &self,
        _: &ConversationObservation,
        _: Option<&NativeObservationBatch>,
    ) -> NativeReceiptInstallation {
        unreachable!()
    }
    fn native_manual_compaction(
        &self,
        _: Option<&crate::runtime::identity::ManualCompactionRequestId>,
        _: Option<&NativeObservationBatch>,
    ) -> NativeReceiptInstallation {
        unreachable!()
    }
    fn native_goal(
        &self,
        _: &crate::durable::inbox::GoalObservationReceipt,
    ) -> NativeReceiptInstallation {
        unreachable!()
    }
    fn native_audit(&self, _: &AttemptId, _: &crate::durable::inbox::PublicationAuditReceipt) {
        unreachable!()
    }
    fn native_pending(
        &self,
        _: Option<&crate::durable::inbox::PendingObservationReceipt>,
        _: &crate::durable::reading::ConversationReadCut,
    ) {
        unreachable!()
    }
    fn opened(&self, _: &AttemptId, _: &crate::publication::PublicationStreamStart) {
        unreachable!()
    }
    fn frame(
        &self,
        _: &AttemptId,
        _: &crate::publication::PublicationFrame,
        _: &crate::publication::PublicationFrameReceipt,
    ) {
        unreachable!()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_readiness_requires_successful_cpu_installation() {
    for accept in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let id = ConversationId::new("conv_fccbe5e9-a68c-75fe-a134-d883c7b1888f");
        let store =
            Arc::new(crate::durable::SqliteConversationStore::in_memory(id.clone()).unwrap());
        let (runtime, _) =
            headless_runtime_over_store(&directory, id.as_str(), store.clone()).await;
        let fanout = ObservationFanout::new(Arc::new(PendingObservations::new()));
        let projection = Arc::new(BatchProjection {
            store: store.clone(),
            installed: AtomicBool::new(false),
            accept,
        });
        fanout.install_publication_projection(projection.clone());
        let attempt = AttemptId::new("installed-first");
        let event = store
            .append_event(attempt_event(
                &id,
                "started",
                &attempt,
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt.clone(),
                },
            ))
            .unwrap();
        let receipt = event.observation.as_deref().unwrap();
        super::super::RuntimeObserver::publish_native_batch(
            &runtime.inner,
            &fanout,
            receipt,
            event.sequence,
        );
        assert_eq!(projection.installed.load(Ordering::Acquire), accept);
        assert_eq!(
            store.observation_ready_frontier(0, event.sequence).unwrap(),
            if accept { event.sequence } else { 0 }
        );
        assert_eq!(
            store
                .observation_event_witness(event.sequence, event.sequence)
                .unwrap()
                .unwrap()
                .native_direct,
            accept
        );
    }
}
