//! Native admission half of the root-local Session title consumer. The model
//! and resource generation are frozen at the existing coordinator boundary.

use super::{ConversationRuntime, RuntimeResourceSnapshot};
use crate::context::{ContextError, ContextErrorKind};
use crate::runtime::auxiliary::executor::{AuxiliaryReservation, AuxiliaryScope};
use crate::runtime::types::{ConversationLifecycle, ConversationLifecycleState};
#[cfg(test)]
use crate::{message::types::MessageBlock, runtime::observation::ConversationObservation};
use std::sync::Arc;

pub(crate) struct PreparedTitle {
    pub(crate) reservation: AuxiliaryReservation,
    pub(crate) scope: Arc<AuxiliaryScope>,
    pub(crate) invocation: crate::model::ResolvedModelInvocation,
    pub(crate) timeout: crate::model::ModelTimeoutPolicy,
    pub(crate) resource_revision: u64,
    // The business owner remains counted after the physical Call retires.
    pub(crate) _admission: crate::runtime::types::LifecycleAdmission,
    pub(crate) _resources: Arc<RuntimeResourceSnapshot>,
}

impl ConversationRuntime {
    pub(crate) fn title_capacity_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.inner.auxiliary.capacity_changes()
    }

    pub(crate) fn title_lifecycle(&self) -> ConversationLifecycle {
        self.inner.lifecycle.clone()
    }

    pub(crate) fn first_title_source(
        &self,
    ) -> Result<
        Option<crate::durable::store::canonical::FirstHumanSource>,
        crate::durable::ConversationStoreError,
    > {
        #[cfg(test)]
        if let Some(reads) = self
            .inner
            .probe
            .lock()
            .expect("coordinator probe")
            .as_ref()
            .and_then(|probe| probe.title_source_reads.as_ref())
        {
            reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        self.inner.store.first_human_source()
    }

    /// Exact terminal and request facts distinguish a released zero-request Call
    /// from preflight rejection; mere absence never confirms an uncertain write.
    pub(crate) fn title_zero_request_settled(
        &self,
        call_id: &crate::runtime::identity::AuxiliaryCallId,
    ) -> Result<bool, crate::durable::ConversationStoreError> {
        let cut = self.inner.store.auxiliary_audit_cut()?;
        let Some(call) = self.inner.store.load_auxiliary_call(call_id, cut)? else {
            return Ok(false);
        };
        Ok(call.settled_sequence.is_some()
            && self
                .inner
                .store
                .read_auxiliary_call_requests(call_id, cut, None, 1)?
                .is_empty())
    }

    /// Composition has excluded the old runtime owner before this is called.
    /// Recovery may retire a stranded zero-request Call, never replay one.
    pub(crate) fn recover_title_source(&self) -> Result<(), ()> {
        if let Some(source) = self.inner.store.first_human_source().map_err(|_| ())?
            && let Some(binding) =
                crate::durable::auxiliary::AuxiliaryTitleSourceBinding::new(&source)
        {
            self.inner
                .store
                .recover_auxiliary_title_source(&binding, self.inner.clock.now())
                .map_err(|_| ())?;
        }
        Ok(())
    }

    pub(crate) fn prepare_title(
        &self,
        cancellation: &crate::runtime::cancellation::CancellationSignal,
    ) -> Result<PreparedTitle, ContextError> {
        let state = self.inner.lock_state();
        let invocation = state.model.title_invocation().map_err(|_| {
            ContextError::new(
                ContextErrorKind::AuxiliaryNotStarted,
                "Title model unavailable",
            )
        })?;
        let scope = self.inner.auxiliary.general_scope(3, cancellation);
        let reservation = self.inner.auxiliary.reserve_general(&scope)?;
        let admission =
            self.inner.lifecycle.try_enter_running().map_err(|_| {
                ContextError::new(ContextErrorKind::Cancelled, "Title runtime closed")
            })?;
        let resources = state.resources.clone();
        let timeout =
            resources
                .configuration()
                .map_or(self.inner.model_timeout_policy, |generation| {
                    generation
                        .config
                        .timeout_policy()
                        .expect("validated configuration generation")
                });
        Ok(PreparedTitle {
            reservation,
            scope,
            invocation,
            timeout,
            resource_revision: resources.revision().get(),
            _admission: admission,
            _resources: resources,
        })
    }

    /// Installed once while inert. The task itself retains only a weak runtime
    /// between operations, so an idle watcher cannot pin a Store allocation.
    pub(crate) fn own_title_task(&self, task: tokio::task::JoinHandle<()>) {
        assert_eq!(
            self.inner.lifecycle.state(),
            ConversationLifecycleState::Inactive
        );
        let mut slot = self
            .inner
            .session_title_task
            .lock()
            .expect("Session title owner");
        assert!(slot.is_none(), "one Title owner per root runtime");
        *slot = Some(task);
    }

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
        let task = self.inner.session_title_task.lock().unwrap().take();
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
            .session_title_task
            .lock()
            .expect("Session title owner")
            .take();
        if let Some(task) = task {
            task.await.expect("Title owner exits normally");
        }
    }
}
