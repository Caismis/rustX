//! Notification fan-in for one connection: level-triggered Session metadata,
//! configuration application and source repair notices, then routed
//! attachment events. One reader drives it directly, without pump tasks.
use std::sync::Arc;

use super::{AppServerConnection, Route, release_route};
use crate::app_server::protocol::{JsonRpcVersion, Method, Notification, NotificationMethod};
use crate::local_runtime::configuration::application::ApplicationRead;
use crate::runtime_client::host::EventDelivery;

#[allow(clippy::large_enum_variant)] // The existing bounded Projection event stays inline through fan-in.
enum RoutedDelivery {
    Projection(EventDelivery),
    Auxiliary(crate::runtime::auxiliary::observer::AuxiliarySegment),
}

/// One application read survives cancelled transport select turns. Keeping
/// its handle here bounds physical lock waiters to one per connection.
#[derive(Default)]
pub(super) struct ConfigurationInterest {
    versions: std::collections::BTreeMap<String, u64>,
    read_version: u64,
    reading: Option<
        tokio::task::JoinHandle<
            Option<crate::local_runtime::configuration::application::ConfigurationApplication>,
        >,
    >,
}

/// Which source repair observations one connection may see.
///
/// The User source always; a Workspace source once this connection addressed
/// it in a request — exactly the sources its own `configuration/sourcesRead`
/// could read — so no other Workspace identity reaches it. Each admission
/// delivers the source's current level once, then every later change; the set
/// is finite and forgets the least recently addressed Workspace first, so
/// addressing it again is a new admission.
pub(super) struct RepairInterest {
    delivered: std::collections::BTreeMap<
        crate::local_runtime::configuration::repair::SourceKey,
        RepairCursor,
    >,
    workspaces: std::collections::VecDeque<std::path::PathBuf>,
}

/// Where one admitted source's observations stand for its connection.
#[derive(Clone, Copy)]
enum RepairCursor {
    /// Admitted; its current level — a receipt or none — is still owed.
    Admitted,
    /// Delivered through this native ledger sequence, a level that did or did
    /// not hold a receipt: a delivered receipt the finite ledger later forgets
    /// is owed its absence, a delivered absence nothing more.
    Delivered { sequence: u64, receipt: bool },
}

/// Workspace sources one connection observes for repair notices.
const REPAIR_INTEREST_WORKSPACES: usize = 64;

impl Default for RepairInterest {
    fn default() -> Self {
        Self {
            delivered: [(
                crate::local_runtime::configuration::repair::SourceKey::User,
                RepairCursor::Admitted,
            )]
            .into(),
            workspaces: std::collections::VecDeque::new(),
        }
    }
}

impl RepairInterest {
    /// Admit one Workspace source; whether it was newly admitted. Addressing
    /// an admitted source only makes it the most recently addressed.
    fn observe(&mut self, directory: std::path::PathBuf) -> bool {
        use crate::local_runtime::configuration::repair::SourceKey;
        let admitted =
            if let Some(position) = self.workspaces.iter().position(|known| *known == directory) {
                self.workspaces.remove(position);
                false
            } else {
                self.delivered.insert(
                    SourceKey::Workspace(directory.clone()),
                    RepairCursor::Admitted,
                );
                true
            };
        self.workspaces.push_back(directory);
        if self.workspaces.len() > REPAIR_INTEREST_WORKSPACES
            && let Some(forgotten) = self.workspaces.pop_front()
        {
            self.delivered.remove(&SourceKey::Workspace(forgotten));
        }
        admitted
    }
}

impl AppServerConnection {
    /// Admit the Workspace source a request addresses into this connection's
    /// repair observations, before the request runs: a Session open that
    /// fails on an unrepairable source still announces why.
    ///
    /// Admission is the linearization point: under the interest lock the
    /// source enters owing its current level, and the connection's
    /// notification wakeup is signalled after the lock is released. A parked
    /// reader therefore rechecks and delivers that level with no ledger
    /// change; a reader between its check and its wait keeps the stored
    /// permit. Re-addressing an admitted source changes nothing it delivers.
    pub(super) async fn observe_repair_source(&self, method: &Method) {
        let directory = match method {
            Method::McpCatalog { target, .. }
            | Method::SkillsList { target, .. }
            | Method::SkillsRead { target, .. }
            | Method::SourcesRead { target }
            | Method::SourcesWrite { target, .. }
            | Method::ConfigurationReconcile { target }
            | Method::McpProbe { target, .. } => target
                .validate()
                .ok()
                .and_then(|()| target.workspace().map(std::path::Path::to_path_buf)),
            Method::SessionCreate { settings, .. } => std::fs::canonicalize(&settings.cwd).ok(),
            Method::SessionAttach { thread_id } => {
                if let Ok(thread) = self.sessions.thread(thread_id).await {
                    self.sessions
                        .read_settings(&thread.session_id)
                        .await
                        .ok()
                        .and_then(|(_, settings)| std::fs::canonicalize(&settings.cwd).ok())
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(directory) = directory.filter(|directory| directory.is_dir()) {
            let admitted = self
                .source_repairs
                .lock()
                .expect("source repair interest")
                .observe(directory);
            if admitted {
                self.changed.notify_one();
            }
        }
    }

    /// The next source repair observation this connection has not delivered.
    /// Level-triggered against the native ledger and per-source cursors.
    ///
    /// A newly admitted source delivers its current level — its receipt, or
    /// none, even when the ledger has forgotten it or never held it — at the
    /// ledger sequence it was read at; every later change of it follows. The
    /// level and that sequence are read under one ledger lock, and the cursor
    /// is set to it while the interest lock is held throughout, so a change
    /// committed after that read carries a higher sequence and is delivered
    /// next, and one committed before it is part of the level: nothing is
    /// lost, stale or delivered twice.
    ///
    /// A source this connection keeps observing may still lose its ledger
    /// entry, which the finite ledger forgets only while recording another
    /// source and so advancing the sequence. When the level last delivered
    /// held a receipt, that absence is delivered once, at the sequence it was
    /// read at, so no client keeps a receipt the ledger forgot; a delivered
    /// absence is not repeated however the ledger advances. A later entry of
    /// the source is newer than both and follows.
    pub(crate) fn next_source_repair(&self) -> Option<NotificationMethod> {
        use crate::local_runtime::configuration::repair::{SourceChange, SourceKey};
        use crate::local_runtime::configuration::settings::SourceTarget;
        // A v79 notice reaches only a connection that negotiated v79.
        if self.initialized.lock().expect("initialize mutex").is_none() {
            return None;
        }
        let ledger = self.host.manager().source_repairs();
        let mut interest = self.source_repairs.lock().expect("source repair interest");
        for (key, cursor) in &mut interest.delivered {
            let (sequence, receipt) = match *cursor {
                RepairCursor::Admitted => ledger.level(key),
                RepairCursor::Delivered {
                    sequence: delivered,
                    receipt: held,
                } => match ledger.changed_after(key, delivered) {
                    Some(SourceChange::Entry(sequence, receipt)) => (sequence, receipt),
                    Some(SourceChange::Absent(sequence)) if held && sequence > delivered => {
                        (sequence, None)
                    }
                    _ => continue,
                },
            };
            *cursor = RepairCursor::Delivered {
                sequence,
                receipt: receipt.is_some(),
            };
            let target = match key {
                SourceKey::User => SourceTarget::User,
                SourceKey::Workspace(directory) => SourceTarget::Workspace {
                    directory: directory.clone(),
                },
            };
            return Some(NotificationMethod::SourceRepairChanged {
                target,
                receipt,
                sequence,
            });
        }
        None
    }

    /// Whether this connection still observes `target`'s repairs.
    #[cfg(test)]
    pub(crate) fn observes_repairs_of(
        &self,
        target: &crate::local_runtime::configuration::settings::SourceTarget,
    ) -> bool {
        self.source_repairs
            .lock()
            .expect("source repair interest")
            .delivered
            .contains_key(&target.into())
    }

    /// The next configuration application this connection has not delivered yet.
    ///
    /// Level-triggered against the manager's current applications and this
    /// connection's per-scope version cursor.
    async fn next_configuration_change(&self) -> Option<NotificationMethod> {
        let mut interest = self.configuration_interest.lock().await;
        // Source capture holds the application owner while reading authored
        // bytes. Waiting for that owner must not occupy a transport worker.
        // Advance the connection cursor only after this cancellable read returns.
        let application = loop {
            if interest.reading.is_none() {
                let manager = self.host.manager().clone();
                match manager.try_next_configuration_application(&interest.versions) {
                    ApplicationRead::Ready(Some(application)) => break application,
                    ApplicationRead::Ready(None) => return None,
                    ApplicationRead::Busy => {}
                }
                let versions = interest.versions.clone();
                interest.read_version = *manager.configuration_changes().borrow();
                interest.reading = Some(tokio::task::spawn_blocking(move || {
                    manager.next_configuration_application(&versions)
                }));
            }
            let result = interest.reading.as_mut().expect("application read").await;
            interest.reading = None;
            if let Some(application) = result.ok()? {
                break application;
            }
            // A cancelled poll can leave a completed empty read behind.
            // Recheck it against publications predating this reader's watch
            // subscription before allowing the reader to park again.
            if interest.read_version == *self.host.manager().configuration_changes().borrow() {
                return None;
            }
        };
        let projection =
            crate::local_runtime::configuration::observation::session_application(&application)
                .ok()?;
        interest
            .versions
            .insert(application.scope, application.version);
        Some(NotificationMethod::ConfigurationChanged {
            application: projection,
        })
    }

    fn next_ownership_invalidation(&self) -> Option<NotificationMethod> {
        let mut delivered = self
            .ownership_invalidations_delivered
            .lock()
            .expect("ownership invalidation cursor");
        let (sequence, session_id, retired_through) = self
            .summary_invalidations
            .next_ownership_after(*delivered)?;
        *delivered = sequence;
        Some(NotificationMethod::OwnershipRetired {
            session_id,
            retired_through: retired_through.to_string(),
        })
    }

    fn next_summary_invalidation(&self) -> Option<NotificationMethod> {
        let mut delivered = self
            .summary_invalidations_delivered
            .lock()
            .expect("summary invalidation cursor");
        let (sequence, session_id, catalog_changed) =
            self.summary_invalidations.next_after(*delivered)?;
        *delivered = sequence;
        Some(NotificationMethod::SummaryInvalidated {
            session_id,
            catalog_changed,
        })
    }

    /// The next level-triggered notification owed outside any attachment, in
    /// priority order: Session metadata, configuration application, then
    /// configuration source repair.
    async fn next_level_notification(&self) -> Option<NotificationMethod> {
        if let Some(notification) = self
            .next_ownership_invalidation()
            .or_else(|| self.next_summary_invalidation())
        {
            return Some(notification);
        }
        self.next_configuration_change()
            .await
            .or_else(|| self.next_source_repair())
            .or_else(|| self.next_schedule_change())
    }

    fn next_schedule_change(&self) -> Option<NotificationMethod> {
        if self.initialized.lock().expect("initialize mutex").is_none() {
            return None;
        }
        let mut changes = self.schedule_changes.lock().expect("task catalog changes");
        if !changes.has_changed().unwrap_or(false) {
            return None;
        }
        changes.borrow_and_update();
        Some(NotificationMethod::ScheduleChanged {})
    }

    /// Wait for one routed notification. No per-Session event queues or pump tasks.
    /// # Panics
    /// Panics if a connection routing mutex is poisoned.
    pub async fn next_notification(&self) -> Notification {
        loop {
            let (notification, publication) = tokio::select! {
                event = self.service_events.next() => {
                    if event.fence.as_ref().is_some_and(tokio_util::sync::CancellationToken::is_cancelled) { continue; }
                    let _ = event.emitted.send(());
                    return event.notification;
                },
                candidate = self.next_notification_candidate() => candidate,
            };
            if let Some(publication) = publication {
                let record = crate::app_server::transport::serialize_record(&notification)
                    .expect("bounded native notification");
                if matches!(
                    publication.hand_off(&record, |_| std::task::Poll::Ready(Ok(()))),
                    crate::app_server::registration::Acceptance::Discarded
                ) {
                    continue;
                }
            }
            return notification;
        }
    }

    async fn next_thread_status(&self) -> NotificationMethod {
        if self.initialized.lock().expect("initialize mutex").is_none() {
            return std::future::pending().await;
        }
        let mut events = self.thread_status_events.lock().await;
        match events.recv().await {
            Ok(crate::local_runtime::session_runtime_manager::status::ThreadStatusEvent::Changed(event)) => NotificationMethod::ThreadStatusChanged(event.into()),
            Ok(crate::local_runtime::session_runtime_manager::status::ThreadStatusEvent::ResyncRequired) => NotificationMethod::ThreadStatusResyncRequired {},
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                *events = events.resubscribe();
                NotificationMethod::ThreadStatusResyncRequired {}
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => std::future::pending().await,
        }
    }

    #[allow(clippy::too_many_lines)] // One fair selection across instance and attachment notifications.
    pub(crate) async fn next_notification_candidate(
        &self,
    ) -> (
        Notification,
        Option<crate::app_server::registration::Publication>,
    ) {
        let _reader = self.reader.lock().await;
        let mut configuration_changes = self.host.manager().configuration_changes();
        let mut summary_invalidations = self.summary_invalidations.changes();
        let mut source_repairs = self.host.manager().source_repairs().subscribe();
        let mut schedule_changes = self
            .schedule_changes
            .lock()
            .expect("task catalog changes")
            .clone();
        let level_notification = |notification| {
            (
                Notification {
                    jsonrpc: JsonRpcVersion::V2,
                    notification,
                },
                None,
            )
        };
        loop {
            let levels = self.next_level_notification();
            tokio::pin!(levels);
            // A large source capture can delay an application read. Its
            // pending notification must not hold up ready attachment events.
            let levels_pending = match futures_util::poll!(&mut levels) {
                std::task::Poll::Ready(Some(notification)) => {
                    return level_notification(notification);
                }
                std::task::Poll::Ready(None) => false,
                std::task::Poll::Pending => true,
            };
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let mut routes: Vec<_> = self
                .routes
                .lock()
                .expect("routes mutex")
                .active
                .values()
                .cloned()
                .collect();
            if routes.is_empty() {
                tokio::select! {
                    status=self.next_thread_status()=>return level_notification(status),
                    notification = &mut levels, if levels_pending => {
                        if let Some(notification) = notification {
                            return level_notification(notification);
                        }
                    },
                    () = &mut changed => {},
                    _ = configuration_changes.changed() => {},
                    _ = summary_invalidations.changed() => {},
                    _ = source_repairs.changed() => {},
                    _ = schedule_changes.changed() => {},
                }
                continue;
            }
            // A continuously ready Session cannot starve other attachments.
            let start = self
                .next_route
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                % routes.len();
            routes.rotate_left(start);
            let pending: Vec<_> = routes
                .into_iter()
                .map(|route| {
                    Box::pin(async move {
                        // The exact registration this poll parks on travels with
                        // its delivery: a `Closed` only ends residency if it is
                        // still the attachment's current registration.
                        let (registration, subscription) =
                            route.registration.wait_following().await;
                        let delivery = if route.client.validate().is_ok() {
                            tokio::select! {
                                event = subscription.next() => RoutedDelivery::Projection(event),
                                segment = subscription.next_auxiliary() => RoutedDelivery::Auxiliary(segment),
                            }
                        } else {
                            RoutedDelivery::Projection(EventDelivery::Closed)
                        };
                        (route, delivery, Some(subscription), registration)
                    })
                })
                .collect();
            let (route, delivery, observed, registration) = tokio::select! {
                status=self.next_thread_status()=>return level_notification(status),
                notification = &mut levels, if levels_pending => {
                    if let Some(notification) = notification {
                        return level_notification(notification);
                    }
                    continue;
                },
                () = &mut changed => continue,
                _ = configuration_changes.changed() => continue,
                _ = summary_invalidations.changed() => continue,
                _ = source_repairs.changed() => continue,
                _ = schedule_changes.changed() => continue,
                result = futures_util::future::select_all(pending) => result.0,
            };
            if let Some(candidate) =
                self.routed_notification(&route, delivery, observed, registration)
            {
                return candidate;
            }
        }
    }
    #[allow(clippy::too_many_lines)] // One registration-scoped native projection.
    fn routed_notification(
        &self,
        route: &Arc<Route>,
        delivery: RoutedDelivery,
        observed: Option<crate::runtime_client::host::EventSubscription>,
        registration: crate::app_server::registration::Registration,
    ) -> Option<(
        Notification,
        Option<crate::app_server::registration::Publication>,
    )> {
        let target = route.target.clone();
        let mut publication = None;
        let notification = match delivery {
            RoutedDelivery::Auxiliary(segment) => {
                let access = self
                    .delivery_access
                    .clone()
                    .filter(|access| !access.is_cancelled())?;
                let registration_id = registration.id.clone();
                publication = Some(registration.auxiliary(
                    access,
                    route.clone(),
                    self.host.revocations().clone(),
                ));
                NotificationMethod::Auxiliary {
                    target,
                    registration_id,
                    segment,
                }
            }
            RoutedDelivery::Projection(EventDelivery::Event(event)) => {
                for fact in event.event.facts() {
                    match fact {
                        crate::runtime_client::RuntimeClientEvent::InteractionPending {
                            interaction,
                        } => {
                            if let Ok(owner) = route.attachment.operation_authority() {
                                self.host.callbacks().interaction(
                                    &target.conversation_id,
                                    &interaction.interaction,
                                    &owner,
                                );
                            }
                        }
                        crate::runtime_client::RuntimeClientEvent::InteractionSettled {
                            interaction,
                            ..
                        }
                        | crate::runtime_client::RuntimeClientEvent::InteractionRemoved {
                            interaction,
                        } => {
                            self.host.callbacks().interaction_settled(interaction);
                        }
                        _ => {}
                    }
                }
                let registration_id = registration.id.clone();
                publication = Some(registration.event(event.cursor));
                NotificationMethod::Event {
                    registration_id,
                    target,
                    cursor: event.cursor,
                    event: event.event,
                }
            }
            RoutedDelivery::Projection(EventDelivery::Unavailable { message }) => {
                let registration_id = registration.id.clone();
                let (recovery, _) = registration.recovery();
                publication = Some(recovery);
                NotificationMethod::ObservationUnavailable {
                    registration_id,
                    target,
                    message: message.chars().take(512).collect(),
                }
            }
            RoutedDelivery::Projection(EventDelivery::ResyncRequired {
                after_cursor: _,
                earliest_serviceable,
            }) => {
                let registration_id = registration.id.clone();
                let (recovery, after_cursor) = registration.recovery();
                publication = Some(recovery);
                NotificationMethod::ResyncRequired {
                    registration_id,
                    target,
                    reason: crate::app_server::protocol::ResyncReason::ReplayUnavailable,
                    checkpoint_required: true,
                    failed_at_cursor: None,
                    after_cursor,
                    earliest_serviceable,
                }
            }
            RoutedDelivery::Projection(EventDelivery::Closed)
                if observed
                    .is_some_and(|subscription| route.attachment.superseded(&subscription)) =>
            {
                // A resync replaced this registration while the consumer was
                // parked on it. The attachment is alive and has a newer
                // registration; retiring its route here would end the
                // attachment that asked for the repair.
                return None;
            }
            RoutedDelivery::Projection(EventDelivery::Closed | EventDelivery::Exhausted) => {
                release_route(&self.host, &self.routes, route);
                NotificationMethod::Closed { target }
            }
            RoutedDelivery::Projection(EventDelivery::Pending) => {
                unreachable!("async delivery never returns Pending")
            }
        };
        Some((
            Notification {
                jsonrpc: JsonRpcVersion::V2,
                notification,
            },
            publication,
        ))
    }
}
