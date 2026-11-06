//! Session-owned model selection applied to a resident runtime and persisted
//! through the Session Catalog.

use super::super::session::SessionId;
use super::SessionRuntimeManager;

impl SessionRuntimeManager {
    /// Native ownership starts before capture and survives loss of the RPC.
    /// Capture and commit share the source fence; model resolution holds neither
    /// that fence nor Attempt admission. No source files are ingested here.
    pub(crate) async fn set_model(
        &self,
        session: &SessionId,
        selection: crate::model::session::SessionModelConfig,
    ) -> Result<(crate::model::session::SessionModelView, u64), ModelSelectionError> {
        self.set_model_update(
            session,
            ModelSelectionInput::Configuration(Box::new(selection)),
        )
        .await
    }

    pub(crate) async fn select_catalog_model(
        &self,
        session: &SessionId,
        choice: crate::model::session::CatalogModelSelection,
    ) -> Result<(crate::model::session::SessionModelView, u64), ModelSelectionError> {
        self.set_model_update(session, ModelSelectionInput::Catalog(choice))
            .await
    }

    async fn set_model_update(
        &self,
        session: &SessionId,
        input: ModelSelectionInput,
    ) -> Result<(crate::model::session::SessionModelView, u64), ModelSelectionError> {
        use crate::runtime::conversation_runtime::ModelUpdateError;
        let owner = self.clone();
        let session = session.clone();
        tokio::task::spawn_blocking(move || owner.set_model_update_blocking(&session, input))
            .await
            .map_err(|_| {
                ModelSelectionError::Model(ModelUpdateError::DurabilityFailed {
                    message: "Session model operation interrupted; reread Session authority".into(),
                })
            })?
    }

    fn set_model_update_blocking(
        &self,
        session: &SessionId,
        input: ModelSelectionInput,
    ) -> Result<(crate::model::session::SessionModelView, u64), ModelSelectionError> {
        use crate::runtime::conversation_runtime::ModelUpdateError;
        let owner = self;
        let (runtime, candidate, adopted, expected_settings, source_identity) = {
            let application = owner.applications.lock();
            let catalog = owner.sessions.catalog.blocking_lock();
            let expected_settings = catalog.settings_revision(session)?;
            let adopted = owner
                .sessions
                .configuration_bindings
                .lock()
                .expect("Session configuration bindings")
                .get(session)
                .cloned()
                .ok_or(ModelUpdateError::Inactive)?;
            let runtime = owner
                .configuration_runtime(session)
                .ok_or(ModelUpdateError::Inactive)?;
            let candidate = runtime.capture_model_selection()?;
            if candidate.baseline != adopted.binding_revision {
                return Err(ModelUpdateError::Conflict.into());
            }
            let identity = application.view(session.as_ref()).map(|view| view.desired);
            (runtime, candidate, adopted, expected_settings, identity)
        };
        let selection = match input {
            ModelSelectionInput::Configuration(selection) => *selection,
            ModelSelectionInput::Catalog(choice) => candidate.catalog_selection(&choice)?,
        };
        let candidate = runtime.resolve_model_selection(candidate, selection.clone())?;
        #[cfg(test)]
        owner
            .configuration
            .test_hooks
            .reach("model_selection_prepared");

        let mut application = owner.applications.lock();
        if application.view(session.as_ref()).map(|view| view.desired) != source_identity {
            return Err(ModelUpdateError::Conflict.into());
        }
        let mut catalog = owner.sessions.catalog.blocking_lock();
        if catalog.settings_revision(session)? != expected_settings {
            return Err(ModelUpdateError::Conflict.into());
        }
        let (_, mut settings) = catalog.lineage(session, None)?;
        let selection_is_persisted = settings.model.as_ref() == Some(&selection);
        settings.model = Some(selection.clone());
        let mut persistence_error = None;
        let result = runtime.commit_model_selection(candidate, |revision| {
            if selection_is_persisted && revision == adopted.binding_revision {
                return Ok(false);
            }
            // Linearization: catalog visibility under the runtime admission
            // gate. Before visibility, failure publishes nothing. Once visible,
            // even an uncertain directory fsync must publish the same authority;
            // the caller receives that typed failure, never success or rollback.
            if let Err(error) = catalog.replace_settings(session, expected_settings, settings) {
                let committed = error.committed();
                let diagnostic = error.to_string();
                persistence_error = Some(error);
                if !committed {
                    return Err(ModelUpdateError::DurabilityFailed {
                        message: diagnostic,
                    });
                }
            }
            owner
                .sessions
                .configuration_bindings
                .lock()
                .expect("Session configuration bindings")
                .insert(
                    session.clone(),
                    adopted.with_model_selection(selection.clone(), revision),
                );
            Ok(true)
        });
        let changed = result.as_ref().is_ok_and(|commit| commit.changed);
        if changed {
            application.selection_changed(session.as_ref(), selection);
            owner.applications.notify(&application);
        }
        let settings_revision = result
            .as_ref()
            .ok()
            .map(|_| catalog.settings_revision(session))
            .transpose()?;
        drop(catalog);
        drop(application);
        if changed {
            owner.applications.run(owner.clone());
        }
        if let Some(error) = persistence_error {
            return Err(ModelSelectionError::Persistence(error));
        }
        result
            .map(|commit| {
                (
                    commit.view,
                    settings_revision.expect("confirmed native settings revision"),
                )
            })
            .map_err(ModelSelectionError::Model)
    }
}

/// Internal selection failures map to the existing App Server error contract.
#[derive(Debug)]
pub(crate) enum ModelSelectionError {
    Model(crate::runtime::conversation_runtime::ModelUpdateError),
    Persistence(super::super::session::SessionError),
}

impl From<crate::runtime::conversation_runtime::ModelUpdateError> for ModelSelectionError {
    fn from(error: crate::runtime::conversation_runtime::ModelUpdateError) -> Self {
        Self::Model(error)
    }
}

impl From<super::super::session::SessionError> for ModelSelectionError {
    fn from(error: super::super::session::SessionError) -> Self {
        Self::Persistence(error)
    }
}

/// Native selection inputs share the existing capture/resolve/commit owner.
enum ModelSelectionInput {
    Configuration(Box<crate::model::session::SessionModelConfig>),
    Catalog(crate::model::session::CatalogModelSelection),
}
