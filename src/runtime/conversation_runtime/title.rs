//! Host-task slot and Session title test helpers. The Session title
//! business logic itself lives in `local_runtime::session_title`
//! (Issue #606); admission goes through the shared
//! `ExternalAuxiliary::admit_auxiliary_work` critical section.
#[cfg(test)]
use crate::{message::types::MessageBlock, runtime::observation::ConversationObservation};
#[cfg(test)]
use std::sync::Arc;

use super::ConversationRuntime;

impl ConversationRuntime {
    /// Commit a native canonical fixture receipt through the production source sink.
    #[cfg(test)]
    pub(crate) fn append_title_source_for_test(&self, message: &MessageBlock) {
        let receipt = self
            .inner
            .store
            .append_canonical(message)
            .expect("native fixture canonical commit");
        self.inner
            .observe(ConversationObservation::CanonicalCommitted {
                attempt_id: None,
                receipt,
            });
    }

    #[cfg(test)]
    pub(crate) fn title_metadata_for_test(
        &self,
    ) -> crate::runtime::observation::SessionObservationMetadata {
        self.inner
            .pending
            .get()
            .expect("fixture source installed")
            .session_metadata()
            .borrow()
            .clone()
    }

    #[cfg(test)]
    pub(crate) fn title_metadata_changes_for_test(
        &self,
    ) -> tokio::sync::watch::Receiver<crate::runtime::observation::SessionObservationMetadata> {
        self.inner.pending.get().unwrap().session_metadata()
    }

    #[cfg(test)]
    pub(crate) async fn stop_title_for_test(&self) {
        let task = self.inner.host_task.lock().unwrap().take();
        if let Some(task) = task {
            task.abort();
            let _ = task.await;
        }
    }

    #[cfg(test)]
    pub(crate) fn count_title_reads_for_test(&self) -> Arc<std::sync::atomic::AtomicUsize> {
        let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        self.inner
            .probe
            .lock()
            .unwrap()
            .get_or_insert_with(Default::default)
            .title_source_reads = Some(reads.clone());
        reads
    }

    #[cfg(test)]
    pub(crate) async fn join_title_for_test(&self) {
        let task = self
            .inner
            .host_task
            .lock()
            .expect("Session title owner")
            .take();
        if let Some(task) = task {
            task.await.expect("Title owner exits normally");
        }
    }
}
