//! Finite manager-owned storage flights, separate from runtime residency.
use super::super::session::{SessionId, SessionNodeId};
use super::super::session_controller::SessionAccess;
use super::{
    Entry, Flight, ManagedRuntime, Outcome, RuntimeManagerError, SessionRuntimeManager, error,
};
use crate::durable::SqliteConversationStore;
use crate::scheduler::{
    ScheduleBinding, ScheduleCommand, ScheduleCommandResult, ScheduleId, ScheduleOperation,
    ScheduleOperationResult, ScheduleSnapshot,
};
#[cfg(test)]
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

#[derive(Debug)]
pub(super) struct StorageFlight {
    pub(super) binding: ScheduleBinding,
    pub(super) retirement: Arc<Flight>,
    result: watch::Sender<Option<Result<ScheduleOperationResult, RuntimeManagerError>>>,
    resources: Mutex<StorageResources>,
}
#[derive(Debug, Default)]
struct StorageResources {
    store: Option<SqliteConversationStore>,
    access: Option<SessionAccess>,
}
impl StorageFlight {
    fn new(binding: ScheduleBinding) -> Arc<Self> {
        Arc::new(Self {
            binding,
            retirement: Flight::new(),
            result: watch::channel(None).0,
            resources: Mutex::default(),
        })
    }
    async fn result(&self) -> Result<ScheduleOperationResult, RuntimeManagerError> {
        self.result
            .subscribe()
            .wait_for(Option::is_some)
            .await
            .expect("storage owns sender")
            .as_ref()
            .expect("storage completed")
            .clone()
    }
}
enum Selection {
    Hot(Arc<ManagedRuntime>),
    Wait(Arc<Flight>),
    Cold(Arc<StorageFlight>),
}
struct CancelRead(Arc<crate::durable::sqlite::schedule_read::ScheduleReadScope>);
impl Drop for CancelRead {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
struct CommitPermit(watch::Sender<usize>);
impl Drop for CommitPermit {
    fn drop(&mut self) {
        self.0.send_modify(|n| *n -= 1);
    }
}
impl SessionRuntimeManager {
    pub(crate) async fn scheduled_workflow_recovery(
        &self,
        session: &SessionId,
        node: &SessionNodeId,
        id: crate::runtime::workflow::scheduled::lifecycle::ScheduledWorkflowStartId,
        after: u64,
        limit: u32,
    ) -> Result<
        Option<crate::runtime::workflow::scheduled::lifecycle::ScheduledWorkflowRecovery>,
        RuntimeManagerError,
    > {
        match self
            .schedule_operation(
                session,
                node,
                ScheduleOperation::WorkflowRecovery { id, after, limit },
            )
            .await?
        {
            ScheduleOperationResult::WorkflowRecovery(view) => Ok(view),
            _ => unreachable!("typed Workflow recovery"),
        }
    }
    pub(crate) async fn cancel_scheduled_workflow(
        &self,
        session: &SessionId,
        node: &SessionNodeId,
        id: crate::runtime::workflow::scheduled::lifecycle::ScheduledWorkflowStartId,
    ) -> Result<
        Option<crate::runtime::workflow::scheduled::lifecycle::ScheduledWorkflowRecovery>,
        RuntimeManagerError,
    > {
        match self
            .schedule_operation(session, node, ScheduleOperation::WorkflowCancel { id })
            .await?
        {
            ScheduleOperationResult::WorkflowRecovery(view) => Ok(view),
            _ => unreachable!("typed Workflow cancel"),
        }
    }
    pub(crate) async fn reconcile_scheduled_workflow(
        &self,
        session: &SessionId,
        node: &SessionNodeId,
        id: crate::runtime::workflow::scheduled::lifecycle::ScheduledWorkflowStartId,
        after: u64,
        limit: u32,
    ) -> Result<
        Option<crate::runtime::workflow::scheduled::lifecycle::ScheduledWorkflowRecovery>,
        RuntimeManagerError,
    > {
        match self
            .schedule_operation(
                session,
                node,
                ScheduleOperation::WorkflowReconcile { id, after, limit },
            )
            .await?
        {
            ScheduleOperationResult::WorkflowRecovery(view) => Ok(view),
            _ => unreachable!("typed Workflow reconcile"),
        }
    }

    /// Execute a stable trusted product-control command at the exact native node.
    /// # Errors
    /// Retirement, writer ownership, storage failure and invalid binding are explicit.
    pub async fn schedule_command(
        &self,
        session: &SessionId,
        node: &SessionNodeId,
        command: ScheduleCommand,
    ) -> Result<ScheduleCommandResult, RuntimeManagerError> {
        match self
            .schedule_operation(session, node, ScheduleOperation::Command(command))
            .await?
        {
            ScheduleOperationResult::Command(result) => Ok(result),
            _ => unreachable!("typed schedule command"),
        }
    }
    /// Read a bounded schedule without activating a runtime.
    /// # Errors
    /// Retirement and storage errors refuse access.
    pub async fn schedule_get(
        &self,
        session: &SessionId,
        node: &SessionNodeId,
        id: ScheduleId,
    ) -> Result<Option<ScheduleSnapshot>, RuntimeManagerError> {
        match self
            .schedule_operation(session, node, ScheduleOperation::Get(id))
            .await?
        {
            ScheduleOperationResult::Get(result) => Ok(result),
            _ => unreachable!("typed schedule get"),
        }
    }
    /// Read one bounded ordinal page without activating a runtime.
    /// # Errors
    /// Retirement, invalid page bounds and storage errors refuse access.
    pub async fn schedule_list(
        &self,
        session: &SessionId,
        node: &SessionNodeId,
        after_ordinal: u64,
        limit: u32,
    ) -> Result<Vec<ScheduleSnapshot>, RuntimeManagerError> {
        match self
            .schedule_operation(
                session,
                node,
                ScheduleOperation::List {
                    after_ordinal,
                    limit,
                },
            )
            .await?
        {
            ScheduleOperationResult::List(result) => Ok(result),
            _ => unreachable!("typed schedule list"),
        }
    }
    /// Explicitly reopen and reconcile a cold store after a required-write failure.
    /// An unproven physical writer must first retire through its existing owner.
    /// # Errors
    /// Active runtimes, retirement, incompatible storage and unresolved writers refuse recovery.
    pub async fn recover_schedule_storage(
        &self,
        session: &SessionId,
        node: &SessionNodeId,
    ) -> Result<crate::scheduler::ScheduleSummary, RuntimeManagerError> {
        match self
            .schedule_operation(session, node, ScheduleOperation::Recover)
            .await?
        {
            ScheduleOperationResult::Summary(summary) => Ok(summary),
            _ => unreachable!("typed recovery"),
        }
    }
    async fn schedule_read(
        &self,
        session: &SessionId,
        node: &SessionNodeId,
        operation: ScheduleOperation,
    ) -> Result<ScheduleOperationResult, RuntimeManagerError> {
        let permit = {
            let mut state = self.registry.0.lock().expect("registry mutex");
            if !state.schedule_open || state.retiring_sessions.contains(session) {
                return Err(error("Scheduler admission is closed"));
            }
            if *state.schedule_reads.borrow() >= 64 {
                state.refuse("schedule_read_capacity");
                return Err(error("Scheduler read capacity exhausted"));
            }
            state.schedule_reads.send_modify(|n| *n += 1);
            CommitPermit(state.schedule_reads.clone())
        };
        let scope = Arc::new(crate::durable::sqlite::schedule_read::ScheduleReadScope::default());
        let _cancel = CancelRead(scope.clone());
        let deadline = self
            .clock
            .now_millis()
            .saturating_add(super::schedule_driver::READ_MS);
        let work = async {
            let plan = self
                .sessions
                .catalog
                .lock()
                .await
                .plan_schedule_read(session, node, self.sessions.catalog.clone())
                .map_err(error)?;
            let binding = ScheduleBinding {
                session_id: session.clone(),
                node_id: node.clone(),
                conversation_id: plan.conversation_id.clone(),
            };
            let clock = self.schedule_clock.clone();
            #[cfg(test)]
            // Only explicitly registered read hooks belong in this path. Creating
            // the full per-target fixture here distorts native scale measurements.
            let probe = self
                .registry
                .0
                .lock()
                .expect("registry mutex")
                .probes
                .get(&binding.conversation_id)
                .cloned();
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                #[cfg(test)]
                if let Some(probe) = &probe {
                    tokio::runtime::Handle::current().block_on(probe.schedule_native_read.park());
                }
                scope.check().map_err(error)?;
                let access = plan.open().map_err(error)?;
                scope.check().map_err(error)?;
                let store = SqliteConversationStore::open_schedule_reader(
                    access.conversation_id,
                    &access.database_path,
                    &scope,
                )
                .map_err(error)?
                .with_lifecycle(access.allocation);
                let result = operation
                    .execute(&store, &binding, clock.as_ref())
                    .map_err(error);
                #[cfg(test)]
                if let Some(probe) = probe
                    && probe
                        .pause_schedule_read_return
                        .swap(false, Ordering::SeqCst)
                {
                    tokio::runtime::Handle::current().block_on(probe.schedule_read_return.park());
                }
                result
            })
            .await
            .map_err(error)?
        };
        tokio::select! {biased;
            ()=self.schedule_stop.cancelled()=>Err(error("Scheduler admission is closed")),
            ()=self.clock.wait_until_millis(deadline)=>{
                self.schedule_diagnostics.lock().expect("Scheduler diagnostics").read_timeouts+=1;
                Err(error("Scheduler read budget exceeded"))
            },
            result=work=>result,
        }
    }
    pub(super) fn note_schedule_change(
        &self,
        binding: &ScheduleBinding,
        result: &ScheduleOperationResult,
    ) {
        let changed = matches!(
            result,
            ScheduleOperationResult::Command(
                crate::scheduler::ScheduleCommandResult::Applied { .. }
                    | crate::scheduler::ScheduleCommandResult::Deleted { .. }
            ) | ScheduleOperationResult::Admission(
                crate::scheduler::ScheduleAdmission::Admitted { .. }
                    | crate::scheduler::ScheduleAdmission::WorkflowAdmitted { .. }
                    | crate::scheduler::ScheduleAdmission::Disabled { .. }
            )
        ) || matches!(result,ScheduleOperationResult::Pruned{removed,..} if *removed>0);
        if !changed {
            return;
        }
        let dropped = {
            let mut dirty = self.schedule_dirty.lock().expect("Scheduler dirty set");
            if dirty.len() < 1024 || dirty.contains_key(&binding.conversation_id) {
                dirty.insert(binding.conversation_id.clone(), binding.clone());
                false
            } else {
                true
            }
        };
        if dropped {
            self.schedule_diagnostics
                .lock()
                .expect("Scheduler diagnostics")
                .dropped_hints += 1;
        }
        self.schedule_changed.notify_one();
    }
    pub(crate) fn close_schedule_admission(&self) {
        self.registry
            .0
            .lock()
            .expect("registry mutex")
            .schedule_open = false;
        self.schedule_stop.cancel();
        self.schedule_changed.notify_one();
    }
    fn schedule_commit_permit(
        &self,
        binding: &ScheduleBinding,
        recovering: bool,
    ) -> Result<CommitPermit, RuntimeManagerError> {
        let state = self.registry.0.lock().expect("registry mutex");
        if !state.schedule_open
            || state.retiring_sessions.contains(&binding.session_id)
            || !state.entries.contains_key(&binding.conversation_id)
        {
            return Err(error("Scheduler admission is closed"));
        }
        if !recovering
            && state
                .schedule_write_fenced
                .contains(&binding.conversation_id)
        {
            return Err(error("Scheduler store requires explicit native recovery"));
        }
        state.schedule_commits.send_modify(|n| *n += 1);
        Ok(CommitPermit(state.schedule_commits.clone()))
    }
    #[allow(clippy::too_many_lines)] // Selection, registered ownership and owned reply stay in one protocol.
    pub(crate) async fn schedule_operation(
        &self,
        session: &SessionId,
        node: &SessionNodeId,
        operation: ScheduleOperation,
    ) -> Result<ScheduleOperationResult, RuntimeManagerError> {
        if matches!(
            &operation,
            ScheduleOperation::Get(_)
                | ScheduleOperation::WorkflowRecovery { .. }
                | ScheduleOperation::List { .. }
                | ScheduleOperation::Summary
                | ScheduleOperation::DuePage { .. }
        ) {
            return self.schedule_read(session, node, operation).await;
        }
        loop {
            let target = self
                .sessions
                .resolve_session_target(session, Some(node))
                .await
                .map_err(error)?;
            let binding = ScheduleBinding {
                session_id: session.clone(),
                node_id: target.id,
                conversation_id: target.conversation_id,
            };
            let controller = self
                .sessions
                .catalog
                .lock()
                .await
                .controller()
                .map_err(error)?;
            let selected = {
                let mut state = self.registry.0.lock().expect("registry mutex");
                if !state.schedule_open || state.retiring_sessions.contains(session) {
                    return Err(error("Scheduler admission is closed"));
                }
                if !matches!(&operation, ScheduleOperation::Recover)
                    && state
                        .schedule_write_fenced
                        .contains(&binding.conversation_id)
                {
                    return Err(error("Scheduler store requires explicit native recovery"));
                }
                if matches!(&operation, ScheduleOperation::Recover)
                    && matches!(
                        state.entries.get(&binding.conversation_id),
                        Some(Entry::Loaded(_))
                    )
                {
                    return Err(error("unload the runtime before cold storage recovery"));
                }
                match state.entries.get(&binding.conversation_id) {
                    Some(Entry::Loaded(resident)) => Selection::Hot(resident.identity.clone()),
                    Some(Entry::Loading(flight) | Entry::Unloading { flight, .. }) => {
                        Selection::Wait(flight.clone())
                    }
                    Some(Entry::StorageOnly(flight)) => Selection::Wait(flight.retirement.clone()),
                    None => {
                        if state
                            .entries
                            .values()
                            .filter(|entry| matches!(entry, Entry::StorageOnly(_)))
                            .count()
                            >= 64
                        {
                            state.refuse("schedule_storage_capacity");
                            return Err(error("Scheduler storage flight capacity exhausted"));
                        }
                        let flight = StorageFlight::new(binding.clone());
                        state.entries.insert(
                            binding.conversation_id.clone(),
                            Entry::StorageOnly(flight.clone()),
                        );
                        self.spawn_schedule_storage(
                            flight.clone(),
                            controller.clone(),
                            operation.clone(),
                        );
                        Selection::Cold(flight)
                    }
                }
            };
            match selected {
                Selection::Wait(flight) => {
                    #[cfg(test)]
                    self.probe(&binding.conversation_id)
                        .storage_joined
                        .send_modify(|n| *n += 1);
                    flight.wait().await.operation_result()?;
                }
                Selection::Cold(flight) => return flight.result().await,
                Selection::Hot(identity) => {
                    let lease = match identity.client().admit_operation() {
                        Ok(lease) => lease,
                        Err(RuntimeManagerError::StaleIncarnation) => continue,
                        Err(e) => return Err(e),
                    };
                    let owner = self.clone();
                    let operation = operation.clone();
                    let (sender, receiver) = tokio::sync::oneshot::channel();
                    tokio::spawn(async move {
                        #[cfg(test)]
                        owner
                            .probe(&binding.conversation_id)
                            .before_operation
                            .park()
                            .await;
                        let task = tokio::task::spawn_blocking(move || {
                            let lease = lease;
                            let ownership = controller.ownership_mutation().map_err(error)?;
                            let permit = owner.schedule_commit_permit(&binding, false)?;
                            #[cfg(test)]
                            let busy = owner
                                .probe(&binding.conversation_id)
                                .schedule_writer_busy
                                .clone();
                            #[cfg(test)]
                            let _busy_probe =
                                crate::durable::sqlite::busy_probe::observe(move |count| {
                                    if count == 0 {
                                        busy.send_replace(true);
                                    }
                                });
                            let result = lease.schedule_operation(
                                &binding,
                                &operation,
                                owner.schedule_clock.as_ref(),
                            );
                            drop(permit);
                            drop(ownership);
                            drop(lease);
                            if matches!(&result, Err(RuntimeManagerError::ScheduleStore(_))) {
                                owner
                                    .registry
                                    .0
                                    .lock()
                                    .expect("registry mutex")
                                    .schedule_write_fenced
                                    .insert(binding.conversation_id.clone());
                            }
                            if let Ok(result) = &result {
                                owner.note_schedule_change(&binding, result);
                            }
                            result
                        })
                        .await
                        .map_err(error)
                        .and_then(|result| result);
                        let _ = sender.send(task);
                    });
                    return receiver.await.map_err(error)?;
                }
            }
        }
    }
    #[allow(clippy::too_many_lines)] // Explicit acquire/commit/close/terminal ownership phases.
    fn spawn_schedule_storage(
        &self,
        flight: Arc<StorageFlight>,
        controller: Arc<crate::runtime::local_storage::ProductController>,
        operation: ScheduleOperation,
    ) {
        let owner = self.clone();
        let terminal = StorageTerminal {
            owner: self.clone(),
            flight: flight.clone(),
            finished: false,
            recovering: matches!(&operation, ScheduleOperation::Recover),
        };
        tokio::spawn(async move {
            #[cfg(test)]
            owner
                .probe(&flight.binding.conversation_id)
                .storage_registered
                .park()
                .await;
            let access = owner
                .sessions
                .acquire_session(&flight.binding.session_id, Some(&flight.binding.node_id))
                .await
                .map_err(error);
            let result = match access {
                Err(e) => Err(e),
                Ok(access) => {
                    flight.resources.lock().expect("storage resources").access = Some(access);
                    #[cfg(test)]
                    owner
                        .probe(&flight.binding.conversation_id)
                        .storage_before_open
                        .park()
                        .await;
                    let worker = owner.clone();
                    let current = flight.clone();
                    let opened = tokio::task::spawn_blocking(move || {
                        let mut resources = current.resources.lock().expect("storage resources");
                        let access = resources.access.as_ref().expect("storage allocation");
                        resources.store = Some(
                            SqliteConversationStore::recover_existing(
                                current.binding.conversation_id.clone(),
                                &access.database_path,
                                &controller,
                            )
                            .map_err(RuntimeManagerError::ScheduleStore)?,
                        );
                        Ok::<_, RuntimeManagerError>(())
                    })
                    .await
                    .map_err(error)
                    .and_then(|result| result);
                    if let Err(e) = opened {
                        Err(e)
                    } else {
                        #[cfg(test)]
                        owner
                            .probe(&flight.binding.conversation_id)
                            .storage_before_commit
                            .park()
                            .await;
                        let current = flight.clone();
                        tokio::task::spawn_blocking(move || {
                            let resources = current.resources.lock().expect("storage resources");
                            let _ownership = resources
                                .access
                                .as_ref()
                                .expect("storage allocation")
                                .allocation
                                .ownership_mutation()
                                .map_err(error)?;
                            let _permit = worker.schedule_commit_permit(
                                &current.binding,
                                matches!(&operation, ScheduleOperation::Recover),
                            )?;
                            #[cfg(test)]
                            assert!(
                                !worker
                                    .probe(&current.binding.conversation_id)
                                    .panic_storage_once
                                    .swap(false, Ordering::SeqCst),
                                "injected storage operation panic"
                            );
                            #[cfg(test)]
                            if worker
                                .probe(&current.binding.conversation_id)
                                .fail_storage_accept_once
                                .swap(false, Ordering::SeqCst)
                            {
                                resources
                                    .store
                                    .as_ref()
                                    .expect("storage writer")
                                    .fail_accept_remaining
                                    .store(1, Ordering::SeqCst);
                            }
                            operation
                                .execute(
                                    resources.store.as_ref().expect("storage writer"),
                                    &current.binding,
                                    worker.schedule_clock.as_ref(),
                                )
                                .map_err(RuntimeManagerError::ScheduleStore)
                        })
                        .await
                        .map_err(error)
                        .and_then(|result| result)
                    }
                }
            };
            #[cfg(test)]
            {
                owner
                    .probe(&flight.binding.conversation_id)
                    .storage_after_commit
                    .park()
                    .await;
                owner
                    .probe(&flight.binding.conversation_id)
                    .storage_before_close
                    .park()
                    .await;
            }
            let close_flight = flight.clone();
            #[cfg(test)]
            let fail_close = owner
                .probe(&flight.binding.conversation_id)
                .fail_storage_close_once
                .swap(false, Ordering::SeqCst);
            #[cfg(not(test))]
            let fail_close = false;
            let closed = tokio::task::spawn_blocking(move || {
                if fail_close {
                    return Err(error("injected storage close failure"));
                }
                let mut resources = close_flight
                    .resources
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(store) = resources.store.as_mut() {
                    store.close_storage_writer().map_err(error)?;
                }
                resources.store.take();
                resources.access.take();
                Ok::<_, RuntimeManagerError>(())
            })
            .await
            .map_err(error)
            .and_then(|result| result);
            #[cfg(test)]
            owner
                .probe(&flight.binding.conversation_id)
                .storage_after_release
                .park()
                .await;
            terminal.finish(result, closed);
        });
    }
}
/// Drop cannot claim absence while an allocation or connection remains retained.
struct StorageTerminal {
    recovering: bool,
    owner: SessionRuntimeManager,
    flight: Arc<StorageFlight>,
    finished: bool,
}
impl StorageTerminal {
    fn finish(
        mut self,
        mut result: Result<ScheduleOperationResult, RuntimeManagerError>,
        closed: Result<(), RuntimeManagerError>,
    ) {
        let mut state = self.owner.registry.0.lock().expect("registry mutex");
        if matches!(&result, Err(RuntimeManagerError::ScheduleStore(_))) {
            state
                .schedule_write_fenced
                .insert(self.flight.binding.conversation_id.clone());
        } else if self.recovering && result.is_ok() && closed.is_ok() {
            state
                .schedule_write_fenced
                .remove(&self.flight.binding.conversation_id);
        }
        if self.recovering
            && let Err(e) = &closed
        {
            result = Err(e.clone());
        }
        let retirement = match closed {
            Ok(()) => {
                state.remove(&self.flight.binding.conversation_id);
                Outcome::WriterAbsent(Ok(()))
            }
            Err(e) => {
                state.unload_failures += 1;
                Outcome::RetirementUnproven(e)
            }
        };
        drop(state);
        if let Ok(result) = &result {
            self.owner
                .note_schedule_change(&self.flight.binding, result);
        }
        self.flight.result.send_replace(Some(result));
        self.flight.retirement.result.send_replace(Some(retirement));
        self.finished = true;
    }
}
impl Drop for StorageTerminal {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let resources = self
            .flight
            .resources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let absent = resources.store.is_none() && resources.access.is_none();
        drop(resources);
        let mut state = self.owner.registry.0.lock().expect("registry mutex");
        let e = error("Scheduler storage task terminated before writer retirement");
        let retirement = if absent {
            state.remove(&self.flight.binding.conversation_id);
            Outcome::WriterAbsent(Ok(()))
        } else {
            Outcome::RetirementUnproven(e.clone())
        };
        self.flight.result.send_replace(Some(Err(e)));
        self.flight.retirement.result.send_replace(Some(retirement));
    }
}
