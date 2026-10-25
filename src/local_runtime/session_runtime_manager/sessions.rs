//! Session creation and deletion through the residency owner, so durable
//! Session retirement fences runtime admission on the same registry.

use super::super::session::SessionId;
use super::Outcome;
use super::RuntimeManagerError;
use super::SessionRuntimeManager;
use super::error;

struct CreationReservation(Option<std::sync::Arc<super::RuntimeRegistry>>);
impl CreationReservation {
    fn reserve(
        registry: &std::sync::Arc<super::RuntimeRegistry>,
    ) -> Result<Self, super::super::session::SessionError> {
        let mut state = registry.0.lock().expect("registry mutex");
        if state.draining {
            return Err(super::super::session::SessionError::Catalog {
                detail: "server is draining".into(),
            });
        }
        state
            .reserve()
            .map_err(|error| super::super::session::SessionError::Catalog {
                detail: error.to_string(),
            })?;
        state.pending_creations += 1;
        Ok(Self(Some(registry.clone())))
    }
    fn publish(
        &mut self,
        result: &super::super::session_controller::SessionTransitionResult,
        flight: &std::sync::Arc<super::Flight>,
    ) -> bool {
        let owner = self.0.take().expect("reserved creation");
        let mut registry = owner.0.lock().expect("registry mutex");
        registry.pending_creations -= 1;
        if registry
            .check_session(&result.session.id, &result.session.active_conversation_id)
            .is_err()
        {
            return false;
        }
        registry
            .initialize_loading(flight, &result.session.id, &result.session.active_node)
            .expect("fresh created target is not retired");
        registry.by_session.insert(
            result.session.id.clone(),
            result.session.active_conversation_id.clone(),
        );
        assert!(
            registry
                .entries
                .insert(
                    result.session.active_conversation_id.clone(),
                    super::Entry::Loading(flight.clone())
                )
                .is_none(),
            "creation publishes one fresh residency owner"
        );
        true
    }
}
impl Drop for CreationReservation {
    fn drop(&mut self) {
        if let Some(registry) = self.0.take() {
            registry.0.lock().expect("registry mutex").pending_creations -= 1;
        }
    }
}

impl SessionRuntimeManager {
    pub(crate) async fn create_session(
        &self,
        settings: super::super::session::SessionPersistentState,
    ) -> Result<
        super::super::session_controller::SessionTransitionResult,
        super::super::session::SessionError,
    > {
        self.create_session_inner(settings, None).await
    }
    pub(crate) async fn create_session_submitted(
        &self,
        settings: super::super::session::SessionPersistentState,
        claim: super::super::mcp_drafts::DraftClaim,
    ) -> Result<
        super::super::session_controller::SessionTransitionResult,
        super::super::session::SessionError,
    > {
        self.create_session_inner(settings, Some(claim)).await
    }
    async fn create_session_inner(
        &self,
        mut settings: super::super::session::SessionPersistentState,
        claim: Option<super::super::mcp_drafts::DraftClaim>,
    ) -> Result<
        super::super::session_controller::SessionTransitionResult,
        super::super::session::SessionError,
    > {
        let mut reservation = claim
            .as_ref()
            .map(|_| CreationReservation::reserve(&self.registry))
            .transpose()?;
        let configuration = self.configuration.clone();
        let credentials = self.credentials.clone();
        let input = settings.input();
        let applications = self.applications.clone();
        let owner = self.clone();
        let capture = tokio::task::spawn_blocking(move || {
            owner.repair_session_sources(&input);
            applications
                .lock()
                .initial_binding(&configuration, &input, &credentials, false)
        })
        .await
        .map_err(|error| super::super::session::SessionError::Catalog {
            detail: error.to_string(),
        })?
        .map_err(|detail| super::super::session::SessionError::Catalog { detail })?;
        if let Some(claim) = &claim {
            claim
                .startup
                .bind_policy(capture.config.mcp_startup_policy)
                .map_err(|detail| super::super::session::SessionError::Catalog { detail })?;
        }
        settings.model = capture.input.model.clone();
        let flight = claim.map(|claim| super::Flight::with_startup(claim.into_startup()));
        let mut start = false;
        let result = self
            .sessions
            .create_session_with_binding(settings, Some(capture.clone()), |result| {
                if let (Some(flight), Some(reservation)) = (&flight, &mut reservation) {
                    start = reservation.publish(result, flight);
                }
            })
            .await?;
        if start {
            #[cfg(test)]
            self.configuration.test_hooks.reach("create_before_load");
            self.spawn_load(
                result.session.active_conversation_id.clone(),
                result.session.id.clone(),
                result.session.active_node.clone(),
                flight.expect("submitted creation"),
            );
        }
        #[cfg(test)]
        {
            let hooks = self.configuration.test_hooks.clone();
            // A parked creator must not pin its newly spawned loading flight
            // in Tokio's worker-local LIFO slot.
            tokio::task::spawn_blocking(move || hooks.reach("create_after_visibility"))
                .await
                .expect("creation visibility test hook");
        }
        let mut application = self.applications.lock();
        application.register_session_scope(result.session.id.to_string(), &capture);
        self.applications.notify(&application);
        Ok(result)
    }

    /// Fence runtime admission, prove native writer retirement, then ask the
    /// durable owner to validate and commit the confirmed deletion target.
    /// Cancellation and uncertain retirement leave admission closed.
    pub(crate) async fn delete_session(
        &self,
        session: &SessionId,
        revision: &str,
    ) -> Result<super::super::session::deletion::SessionDeleteResult, RuntimeManagerError> {
        let resident = {
            let mut registry = self.registry.0.lock().expect("registry mutex");
            if !registry.retiring_sessions.insert(session.clone()) {
                return Err(error("Session retirement already owns admission"));
            }
            registry.failed_submissions.retain(|_, flight| {
                let (owner, _) = flight.target.get().expect("failed initialization target");
                owner != session
            });
            registry.by_session.get(session).cloned()
        };
        #[cfg(test)]
        if let Ok(snapshot) = self.sessions.read_session(session).await {
            self.probe(&snapshot.active_conversation_id)
                .after_delete_fence
                .park()
                .await;
        }
        if let Some(id) = resident {
            match self.retire(&id).await {
                Outcome::WriterAbsent(_) => {} // Composition errors cannot undo absence proof.
                Outcome::RetirementUnproven(error) => return Err(error),
                Outcome::Resident(_) => unreachable!("retirement joins every published writer"),
            }
        }
        // WriterAbsent is proof even if composition failed. Only now may catalog
        // deletion acquire destructive Conversation exclusion.
        let result = self
            .sessions
            .delete_session(session, revision)
            .await
            .map_err(error);
        if matches!(
            &result,
            Ok(
                super::super::session::deletion::SessionDeleteResult::Deleted { .. }
                    | super::super::session::deletion::SessionDeleteResult::CommittedCleanupPending { .. }
                    | super::super::session::deletion::SessionDeleteResult::CommittedDurabilityUncertain { .. }
                    | super::super::session::deletion::SessionDeleteResult::NotFound { .. }
            )
        ) {
            self.applications.forget(session.as_ref());
            self.sessions
                .configuration_bindings
                .lock()
                .expect("Session configuration bindings")
                .remove(session);
        }
        if !matches!(
            result,
            Ok(super::super::session::deletion::SessionDeleteResult::CommittedDurabilityUncertain { .. })
        ) {
            self.registry
                .0
                .lock()
                .expect("registry mutex")
                .retiring_sessions
                .remove(session);
        }
        result
    }

    /// Reconcile durable deletion state before retrying committed cleanup.
    /// Live observations never release another delete's admission ownership.
    pub(crate) async fn recover_session_deletion(
        &self,
        session: &SessionId,
    ) -> Result<super::super::session::deletion::SessionDeleteResult, RuntimeManagerError> {
        use super::super::session::deletion::SessionDeleteResult;
        let observed = self.sessions.delete_preview(session).await;
        if !matches!(
            observed,
            SessionDeleteResult::CommittedCleanupPending { .. }
                | SessionDeleteResult::CommittedDurabilityUncertain { .. }
                | SessionDeleteResult::NotFound { .. }
        ) {
            return Ok(observed);
        }
        // Durable recovery cannot substitute for retirement of a managed writer.
        if self
            .registry
            .0
            .lock()
            .expect("registry mutex")
            .by_session
            .contains_key(session)
        {
            return Err(error(
                "writer retirement must be proven before deletion recovery",
            ));
        }
        let result = if matches!(observed, SessionDeleteResult::NotFound { .. }) {
            self.sessions.confirm_deletion_absence(session).await
        } else {
            // These preview outcomes identify an existing frozen deletion record.
            self.sessions.recover_deletion(session).await
        };
        if matches!(
            result,
            SessionDeleteResult::Deleted { .. }
                | SessionDeleteResult::NotFound { .. }
                | SessionDeleteResult::CommittedCleanupPending { .. }
        ) {
            // Durable absence or a durable frozen record now excludes admission.
            // Idempotent, including recovery racing the original cleanup worker.
            self.registry
                .0
                .lock()
                .expect("registry mutex")
                .retiring_sessions
                .remove(session);
        }
        Ok(result)
    }
}
