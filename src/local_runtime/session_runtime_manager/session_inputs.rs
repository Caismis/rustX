//! Validate Session-authored inputs before the catalog CAS, then publish the
//! captured candidate through the existing safe configuration application.
use super::SessionRuntimeManager;
use crate::local_runtime::configuration::application::ApplicationState;
use crate::local_runtime::session::{SessionCatalog, SessionError, SessionPersistentState};
use crate::local_runtime::session_inputs::{
    SessionConfigContent, SessionInputChange, SessionInputReference, SessionInputReferences,
    SessionResources,
};
use crate::runtime::identity::SessionId;
use std::sync::Arc;

#[derive(Debug)]
pub(crate) enum SessionInputError {
    Invalid,
    SourceChanged,
    ExecutionUnavailable,
    Unconfirmed,
    Persistence(SessionError),
}

impl From<SessionError> for SessionInputError {
    fn from(error: SessionError) -> Self {
        Self::Persistence(error)
    }
}

fn referenced_settings(
    catalog: &SessionCatalog,
    workspace: &std::path::Path,
    reference: &SessionInputReference,
) -> Result<SessionPersistentState, SessionInputError> {
    if catalog.settings_revision(&reference.session_id)? != reference.revision {
        return Err(SessionInputError::SourceChanged);
    }
    let (_, settings) = catalog.lineage(&reference.session_id, None)?;
    if settings.cwd.as_os_str() != workspace.as_os_str() {
        return Err(SessionInputError::Invalid);
    }
    Ok(settings)
}

fn resolve_change<T>(
    catalog: &SessionCatalog,
    workspace: &std::path::Path,
    change: SessionInputChange<T>,
    field: impl FnOnce(SessionPersistentState) -> Option<T>,
) -> Result<Option<T>, SessionInputError> {
    match change {
        SessionInputChange::Clear => Ok(None),
        SessionInputChange::Replace(content) => Ok(Some(content)),
        SessionInputChange::Reference(reference) => {
            field(referenced_settings(catalog, workspace, &reference)?)
                .map(Some)
                .ok_or(SessionInputError::Invalid)
        }
    }
}

impl SessionRuntimeManager {
    /// Resolve references under one catalog observation. Only accepted source
    /// bytes are carried into creation and revalidated for the new Session.
    pub(crate) async fn resolve_session_input_references(
        &self,
        mut settings: SessionPersistentState,
        references: Option<SessionInputReferences>,
    ) -> Result<SessionPersistentState, SessionInputError> {
        let Some(references) = references else {
            return Ok(settings);
        };
        let catalog = self.sessions.catalog.lock().await;
        if let Some(reference) = references.session_config {
            if settings.session_config.is_some() {
                return Err(SessionInputError::Invalid);
            }
            settings.session_config = resolve_change(
                &catalog,
                &settings.cwd,
                SessionInputChange::Reference(reference),
                |source| source.session_config,
            )?;
        }
        if let Some(reference) = references.session_resources {
            if settings.session_resources.is_some() {
                return Err(SessionInputError::Invalid);
            }
            settings.session_resources = resolve_change(
                &catalog,
                &settings.cwd,
                SessionInputChange::Reference(reference),
                |source| source.session_resources,
            )?;
        }
        Ok(settings)
    }

    pub(crate) async fn write_session_inputs(
        &self,
        session: SessionId,
        expected: u64,
        config: Option<SessionInputChange<SessionConfigContent>>,
        resources: Option<SessionInputChange<SessionResources>>,
        request: crate::app_server::host::ServerOperation,
    ) -> Result<u64, SessionInputError> {
        let owner = self.clone();
        tokio::task::spawn_blocking(move || {
            let _request = request;
            owner.write_session_inputs_blocking(&session, expected, config, resources)
        })
        .await
        .map_err(|_| SessionInputError::Unconfirmed)?
    }

    #[allow(clippy::too_many_lines)] // One transaction with a validation gap between its two catalog cuts.
    fn write_session_inputs_blocking(
        &self,
        session: &SessionId,
        expected: u64,
        config: Option<SessionInputChange<SessionConfigContent>>,
        resources: Option<SessionInputChange<SessionResources>>,
    ) -> Result<u64, SessionInputError> {
        let (settings, endpoint, retained_model, source_cut) = {
            let application = self.applications.lock();
            let catalog = self.sessions.catalog.blocking_lock();
            let actual = catalog.settings_revision(session)?;
            if actual != expected {
                return Err(SessionError::StaleSettings { expected, actual }.into());
            }
            let (_, mut settings) = catalog.lineage(session, None)?;
            if config.is_none() && resources.is_none() {
                return Err(SessionInputError::Invalid);
            }
            if let Some(config) = config {
                settings.session_config =
                    resolve_change(&catalog, &settings.cwd, config, |source| {
                        source.session_config
                    })?;
            }
            if let Some(resources) = resources {
                settings.session_resources =
                    resolve_change(&catalog, &settings.cwd, resources, |source| {
                        source.session_resources
                    })?;
            }
            let source_cut = application.source_cut(&settings.cwd);
            let endpoint = self
                .configuration_endpoint(session)
                .map_err(|_| SessionInputError::ExecutionUnavailable)?;
            let bindings = self
                .sessions
                .configuration_bindings
                .lock()
                .expect("Session configuration bindings");
            let retained = bindings.get(session);
            let retained_model = retained.map(|binding| binding.input.model.clone());
            (settings, endpoint, retained_model, source_cut)
        };
        let mut input = settings.input();
        if let Some(model) = retained_model {
            input.model = model;
        }
        if endpoint
            .as_ref()
            .is_some_and(|endpoint| endpoint.control().is_none())
        {
            return Err(SessionInputError::ExecutionUnavailable);
        }
        let capture = self.configuration.capture_session_application(
            &input,
            &self.credentials,
            endpoint.as_ref(),
        );
        if endpoint
            .as_ref()
            .is_some_and(|endpoint| endpoint.control().is_none())
        {
            return Err(SessionInputError::ExecutionUnavailable);
        }
        let capture = capture.map_err(|_| SessionInputError::Invalid)?;
        let context = capture
            .context
            .as_ref()
            .map_err(|_| SessionInputError::Invalid)?;
        ApplicationState::validate_selection(context, &self.credentials, true)
            .map_err(|_| SessionInputError::Invalid)?;

        let mut application = self.applications.lock();
        let current_endpoint = self
            .configuration_endpoint(session)
            .map_err(|_| SessionInputError::ExecutionUnavailable)?;
        let same_endpoint = match (&endpoint, &current_endpoint) {
            (None, None) => true,
            (Some(before), Some(after)) => Arc::ptr_eq(before, after),
            _ => false,
        };
        if !same_endpoint || application.source_cut(&input.cwd) != source_cut {
            return Err(SessionInputError::SourceChanged);
        }
        let mut catalog = self.sessions.catalog.blocking_lock();
        let actual = catalog.settings_revision(session)?;
        if actual != expected {
            return Err(SessionError::StaleSettings { expected, actual }.into());
        }
        if catalog.lineage(session, None)?.1 == settings {
            return Ok(actual);
        }
        let revision = catalog.replace_settings(session, expected, settings);
        if revision.is_ok() || revision.as_ref().is_err_and(SessionError::committed) {
            application.cancel_bound_capture(session);
            application.capture_bound_source(session.to_string(), &input.cwd, Ok(capture));
            self.applications.notify(&application);
            drop(catalog);
            drop(application);
            self.applications.run(self.clone());
        }
        revision.map_err(SessionInputError::Persistence)
    }
}
