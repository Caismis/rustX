//! The non-owning managed-runtime client: registry-admitted operation leases
//! and configuration observation sources. Dropping any of them has no runtime
//! side effects beyond releasing its own lease.

use super::super::session::SessionId;
use super::Entry;
use super::ManagedRuntime;
use super::ResidentRuntime;
use super::RuntimeManagerError;
use super::RuntimeResidencyPin;
use super::SessionRuntimeManager;
use super::error;
use super::{CompactionReply, CompactionTail};
use std::sync::Arc;
use std::sync::Weak;
use std::sync::atomic::Ordering;

/// Private, non-cloneable server-operation ownership. Never lent to a client.
pub(crate) struct OperationLease(Option<Arc<ResidentRuntime>>);

impl OperationLease {
    pub(super) fn schedule_operation(
        &self,
        binding: &crate::scheduler::ScheduleBinding,
        operation: &crate::scheduler::ScheduleOperation,
        clock: &dyn crate::runtime::types::RuntimeClock,
    ) -> Result<crate::scheduler::ScheduleOperationResult, RuntimeManagerError> {
        self.0
            .as_ref()
            .expect("operation resident")
            .composition
            .lock()
            .expect("composition mutex")
            .as_ref()
            .ok_or(RuntimeManagerError::StaleIncarnation)?
            .runtime()
            .schedule_operation(binding, operation, clock)
            .map_err(|e| match e {
                crate::runtime::inbound::MailboxError::Durable(e) => {
                    RuntimeManagerError::ScheduleStore(e)
                }
                e => error(e),
            })
    }

    /// Select the already-admitted incarnation without retaining another owner.
    pub(crate) fn configuration_source(&self) -> ConfigurationObservationSource {
        ConfigurationObservationSource {
            resident: Arc::downgrade(self.0.as_ref().expect("operation resident")),
        }
    }
}

/// A read selector lent only to the server operation whose lease owns its
/// lifetime. It cannot admit work, extend residency, or expose a runtime handle.
pub(crate) struct ConfigurationObservationSource {
    resident: Weak<ResidentRuntime>,
}

impl ConfigurationObservationSource {
    /// Preserve the original native application publication fence while reading
    /// the admitted incarnation, including while registry retirement drains it.
    pub(crate) fn read(
        &self,
        manager: &SessionRuntimeManager,
        session: &SessionId,
    ) -> Option<super::super::configuration::settings::EffectiveConfiguration> {
        let resident = self.resident.upgrade()?;
        if !Weak::ptr_eq(
            &resident.identity.registry,
            &Arc::downgrade(&manager.registry),
        ) {
            return None;
        }
        let application = manager.applications.lock();
        let composition = resident.composition.lock().expect("composition mutex");
        let mut view =
            crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
                composition.as_ref()?.runtime(),
            )?;
        view.application = application.view(session.as_ref());
        view.process_bindings = Some(manager.process_policy());
        Some(view)
    }
}

impl Drop for OperationLease {
    fn drop(&mut self) {
        let resident = self.0.take().expect("operation resident");
        let operations = resident.operations.clone();
        // Release strong ownership before publishing the drain acknowledgement.
        drop(resident);
        operations.send_modify(|count| *count -= 1);
    }
}

/// Non-owning control/observation seam. Dropping it has no runtime side effects.
/// Operations return owned facts, never runtime/host/storage handles. The local
/// composition mutex covers bounded synchronous helpers, without await. App
/// Server work uses a registry-admitted server task and scoped operation lease;
/// unload drains these before native shutdown and composition release. No
/// global registry lock covers runtime work or an asynchronous wait.
#[derive(Clone, Debug)]
pub struct ManagedRuntimeClient {
    pub(super) runtime: Weak<ManagedRuntime>,
}

impl ManagedRuntimeClient {
    pub(super) fn admit_operation(&self) -> Result<OperationLease, RuntimeManagerError> {
        let identity = self
            .runtime
            .upgrade()
            .ok_or(RuntimeManagerError::StaleIncarnation)?;
        let registry = identity
            .registry
            .upgrade()
            .ok_or(RuntimeManagerError::StaleIncarnation)?;
        let state = registry.0.lock().expect("registry mutex");
        let Some(Entry::Loaded(resident)) = state.entries.get(&identity.conversation) else {
            return Err(RuntimeManagerError::StaleIncarnation);
        };
        if state.fenced(&identity.conversation)
            || resident.identity.incarnation != identity.incarnation
        {
            return Err(RuntimeManagerError::StaleIncarnation);
        }
        // Same lock as Loaded -> Unloading: no late increment is possible.
        resident.activity.fetch_add(1, Ordering::Relaxed);
        resident.operations.send_modify(|count| *count += 1);
        Ok(OperationLease(Some(resident.clone())))
    }

    /// Admit work to a server-owned task. The caller receives only a reply channel;
    /// cancellation or retention of that receiver cannot retain the operation lease.
    pub(crate) fn start_operation<T, F, Fut>(
        &self,
        operation: F,
    ) -> Result<tokio::sync::oneshot::Receiver<T>, RuntimeManagerError>
    where
        T: Send + 'static,
        F: FnOnce(&OperationLease) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = T> + Send + 'static,
    {
        let lease = self.admit_operation()?;
        #[cfg(test)]
        let probe = {
            let identity = &lease.0.as_ref().expect("operation resident").identity;
            identity
                .registry
                .upgrade()
                .expect("resident registry")
                .0
                .lock()
                .expect("registry mutex")
                .probes
                .get(&identity.conversation)
                .cloned()
        };
        // The synchronous factory selects only the original admitted owner;
        // it cannot borrow this lease into its static execution future. Full
        // reads run later in the server task, whose lease covers retirement.
        let operation = operation(&lease);
        let (sender, receiver) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            #[cfg(test)]
            if let Some(probe) = probe {
                probe.before_operation.park().await;
            }
            let result = operation.await;
            drop(lease);
            let _ = sender.send(result);
        });
        Ok(receiver)
    }

    /// Compaction transfers its whole business tail to `ConversationRuntime`.
    /// The manager lease covers native admission only; the reply cannot pin
    /// residency or prevent an explicit unload from draining that native tail.
    pub(crate) fn start_compaction_operation<F, Fut>(
        &self,
        admission: F,
    ) -> Result<tokio::sync::oneshot::Receiver<CompactionReply>, RuntimeManagerError>
    where
        F: FnOnce(&OperationLease) -> Fut + Send + 'static,
        Fut: std::future::Future<
                Output = Result<CompactionTail, crate::runtime_client::types::RuntimeClientError>,
            > + Send
            + 'static,
    {
        let lease = self.admit_operation()?;
        #[cfg(test)]
        let probe = {
            let identity = &lease.0.as_ref().expect("operation resident").identity;
            identity
                .registry
                .upgrade()
                .expect("resident registry")
                .0
                .lock()
                .expect("registry mutex")
                .probes
                .get(&identity.conversation)
                .cloned()
        };
        let admitted = admission(&lease);
        let (sender, receiver) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            #[cfg(test)]
            if let Some(probe) = probe {
                probe.before_operation.park().await;
            }
            let tail = admitted.await;
            drop(lease);
            let result = match tail {
                Ok(tail) => tail.await,
                Err(error) => Err(error),
            };
            let _ = sender.send(result);
        });
        Ok(receiver)
    }
    /// Reject use of an unloaded, unloading or replaced incarnation.
    /// # Errors
    /// Returns `StaleIncarnation` when residency has ended.
    pub fn validate(&self) -> Result<(), RuntimeManagerError> {
        self.current().map(|_| ())
    }

    /// Admit a non-owning controller and linearize its snapshot/subscription.
    /// # Errors
    /// Stale incarnations and conflicting controllers are rejected.
    /// # Panics
    /// Panics if a composition mutex is poisoned.
    pub(crate) fn attach(
        &self,
    ) -> Result<
        (
            crate::runtime_client::attachment::RuntimeAttachment,
            RuntimeResidencyPin,
        ),
        RuntimeManagerError,
    > {
        let lease = self.admit_operation()?;
        let runtime = lease.0.as_ref().expect("operation resident");
        let registry = runtime
            .identity
            .registry
            .upgrade()
            .ok_or(RuntimeManagerError::StaleIncarnation)?;
        let state = registry.0.lock().expect("registry mutex");
        if state.fenced(&runtime.identity.conversation)
            || !matches!(state.entries.get(&runtime.identity.conversation), Some(Entry::Loaded(current)) if Arc::ptr_eq(current, runtime))
        {
            return Err(RuntimeManagerError::StaleIncarnation);
        }
        // Pin acquisition shares registry publication with idle eviction.
        runtime.external.fetch_add(1, Ordering::Relaxed);
        let external = RuntimeResidencyPin {
            registry: Arc::downgrade(&registry),
            resident: Arc::downgrade(runtime),
            released: std::sync::atomic::AtomicBool::new(false),
        };
        drop(state);
        let composition = runtime.composition.lock().expect("composition mutex");
        let snapshot = composition
            .as_ref()
            .ok_or(RuntimeManagerError::StaleIncarnation)?
            .host()
            .inner
            .admit_attachment_identity(false)
            .map_err(RuntimeManagerError::Client)?;
        Ok((snapshot, external))
    }
    pub(super) fn current(&self) -> Result<Arc<ResidentRuntime>, RuntimeManagerError> {
        let runtime = self
            .runtime
            .upgrade()
            .ok_or(RuntimeManagerError::StaleIncarnation)?;
        let registry = runtime
            .registry
            .upgrade()
            .ok_or(RuntimeManagerError::StaleIncarnation)?;
        let state = registry.0.lock().expect("registry mutex");
        let current = !state.fenced(&runtime.conversation)
            && matches!(state.entries.get(&runtime.conversation),
            Some(Entry::Loaded(resident)) if resident.identity.incarnation == runtime.incarnation);
        if current {
            runtime
                .resident
                .upgrade()
                .ok_or(RuntimeManagerError::StaleIncarnation)
        } else {
            Err(RuntimeManagerError::StaleIncarnation)
        }
    }

    /// Submit through the existing native admission gate.
    /// # Errors
    /// Rejects stale incarnations and native admission failures.
    /// # Panics
    /// Panics if an internal residency mutex was poisoned.
    pub fn submit_inbound(
        &self,
        content: Vec<crate::message::types::UserContentBlock>,
    ) -> Result<crate::runtime::conversation_runtime::InboundAdmission, RuntimeManagerError> {
        if content
            .iter()
            .any(|block| !matches!(block, crate::message::types::UserContentBlock::Text(_)))
        {
            return Err(error("user uploads require server-issued Session receipts"));
        }
        let lease = self.admit_operation()?;
        let runtime = lease.0.as_ref().expect("operation resident");
        let composition = runtime.composition.lock().expect("composition mutex");
        composition
            .as_ref()
            .ok_or(RuntimeManagerError::StaleIncarnation)?
            .runtime()
            .submit_inbound(content)
            .map_err(|e| error(format!("{e:?}")))
    }

    /// Read owned projection facts from the existing Runtime Client host.
    /// # Errors
    /// Rejects stale incarnations and projection failures.
    /// # Panics
    /// Panics if an internal residency mutex was poisoned.
    pub fn checkpoint(
        &self,
    ) -> Result<crate::runtime_client::checkpoint::RuntimeClientCheckpoint, RuntimeManagerError>
    {
        let lease = self.admit_operation()?;
        let runtime = lease.0.as_ref().expect("operation resident");
        let composition = runtime.composition.lock().expect("composition mutex");
        composition
            .as_ref()
            .ok_or(RuntimeManagerError::StaleIncarnation)?
            .host()
            .checkpoint()
            .map_err(|e| error(format!("{e:?}")))
    }
}
