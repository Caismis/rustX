//! Session creation and deletion through the residency owner, so durable
//! Session retirement fences runtime admission on the same registry.

use super::super::session::SessionId;
use super::Outcome;
use super::RuntimeManagerError;
use super::SessionRuntimeManager;
use super::error;

impl SessionRuntimeManager {
    pub(crate) async fn create_session(
        &self,
        mut settings: super::super::session::SessionPersistentState,
    ) -> Result<
        super::super::session_controller::SessionTransitionResult,
        super::super::session::SessionError,
    > {
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
        settings.model = capture.input.model.clone();
        let result = self
            .sessions
            .create_session_with_binding(settings, Some(capture.clone()))
            .await?;
        #[cfg(test)]
        self.configuration
            .test_hooks
            .reach("create_after_visibility");
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
        if let blocked @ super::super::session::deletion::SessionDeleteResult::Blocked { .. } =
            self.sessions.delete_preview(session).await
        {
            return Ok(blocked);
        }
        let residents = {
            let mut registry = self.registry.0.lock().expect("registry mutex");
            if !registry.retiring_sessions.insert(session.clone()) {
                return Err(error("Session retirement already owns admission"));
            }
            registry.session_entries(session)
        };
        #[cfg(test)]
        if let Ok(snapshot) = self.sessions.read_session(session).await {
            self.probe(&snapshot.active_conversation_id)
                .after_delete_fence
                .park()
                .await;
        }
        for id in residents {
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
        if !self
            .registry
            .0
            .lock()
            .expect("registry mutex")
            .session_entries(session)
            .is_empty()
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
