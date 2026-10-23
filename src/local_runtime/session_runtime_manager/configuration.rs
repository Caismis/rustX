//! Configuration adoption and source settings for resident runtimes: test
//! gates, process limits, configuration hosts and applications, adoption,
//! reconciliation, and source repair/read/write.

use super::super::session::SessionId;
use super::Entry;
use super::SessionRuntimeManager;
use crate::runtime::conversation_runtime::ConversationRuntime;
use tokio::sync::watch;

impl SessionRuntimeManager {
    /// Parks a Session application just before it takes the publication
    /// fence that settles an unchanged invocation, so a test can overtake it.
    #[cfg(test)]
    pub(in crate::local_runtime) async fn configuration_catalog_gate(&self, session: &SessionId) {
        let Ok((node, _)) = self.sessions.catalog.lock().await.lineage(session, None) else {
            return;
        };
        let probe = self.probe(&node.conversation_id);
        probe.before_catalog_publication.park().await;
    }

    #[cfg(test)]
    pub(in crate::local_runtime) async fn configuration_test_gate(
        &self,
        session: &SessionId,
        publication: bool,
    ) -> bool {
        let node = self
            .sessions
            .catalog
            .lock()
            .await
            .lineage(session, None)
            .expect("test Session")
            .0;
        let probe = self.probe(&node.conversation_id);
        if publication {
            probe.before_configuration_publish.park().await;
            false
        } else {
            probe
                .configuration_preparations
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            probe.before_configuration_prepare.park().await;
            probe
                .fail_configuration_once
                .swap(false, std::sync::atomic::Ordering::SeqCst)
        }
    }

    pub(crate) fn process_policy(&self) -> super::super::app_server_policy::AppServerPolicy {
        self.process_policy.read().expect("process policy").clone()
    }

    pub(crate) fn bind_process_policy(
        &self,
        policy: super::super::app_server_policy::AppServerPolicy,
    ) {
        *self.process_policy.write().expect("process policy") = policy;
    }

    pub(in crate::local_runtime) fn apply_process_limits(
        &self,
        desired: &super::super::app_server_policy::AppServerPolicy,
    ) -> bool {
        let mut actual = self.process_policy.write().expect("process policy");
        let restart = actual.shutdown_deadline_ms != desired.shutdown_deadline_ms;
        actual.max_connections = desired.max_connections;
        actual.max_external_attachments = desired.max_external_attachments;
        actual.max_resident_runtimes = desired.max_resident_runtimes;
        actual.idle_grace_ms = desired.idle_grace_ms;
        let mut registry = self.registry.0.lock().expect("registry mutex");
        registry.policy.max_resident_runtimes = desired.max_resident_runtimes;
        registry.policy.idle_grace_ms = desired.idle_grace_ms;
        restart
    }

    pub(in crate::local_runtime) fn configuration_runtime(
        &self,
        id: &SessionId,
    ) -> Option<ConversationRuntime> {
        let resident = {
            let state = self.registry.0.lock().expect("registry mutex");
            state.by_session.get(id).and_then(|conversation| {
                match state.entries.get(conversation) {
                    Some(Entry::Loaded(resident)) => Some(resident.clone()),
                    _ => None,
                }
            })
        };
        resident.and_then(|resident| resident.shutdown_runtime())
    }

    /// The resident runtime together with the resource preparation of the
    /// same composition, read under one composition lock.
    pub(in crate::local_runtime) fn configuration_preparation(
        &self,
        id: &SessionId,
    ) -> Option<(
        ConversationRuntime,
        std::sync::Arc<crate::local_runtime::composition::LocalRuntimeResourceLoader>,
    )> {
        let resident = {
            let state = self.registry.0.lock().expect("registry mutex");
            state.by_session.get(id).and_then(|conversation| {
                match state.entries.get(conversation) {
                    Some(Entry::Loaded(resident)) => Some(resident.clone()),
                    _ => None,
                }
            })
        }?;
        let composition = resident.composition.lock().expect("composition mutex");
        let composition = composition.as_ref()?;
        Some((
            composition.runtime().clone(),
            std::sync::Arc::clone(composition.resource_loader()?),
        ))
    }

    /// The resident Session's Runtime Client host, for observing the
    /// projection a client attachment would observe.
    #[cfg(test)]
    pub(crate) fn configuration_host(
        &self,
        id: &SessionId,
    ) -> Option<crate::runtime_client::RuntimeClientHost> {
        let state = self.registry.0.lock().expect("registry mutex");
        let conversation = state.by_session.get(id)?;
        let Some(Entry::Loaded(resident)) = state.entries.get(conversation) else {
            return None;
        };
        resident
            .composition
            .lock()
            .expect("composition mutex")
            .as_ref()
            .map(|composition| composition.host().clone())
    }

    pub(crate) fn configuration_application(
        &self,
        id: &SessionId,
    ) -> Option<super::super::configuration::application::ConfigurationApplication> {
        self.applications.lock().view(id.as_ref())
    }

    /// Independent native configuration oracle for bounded serializer tests.
    /// Server reads instead use the original admitted operation's source.
    #[cfg(test)]
    pub(crate) fn effective_configuration_observation(
        &self,
        id: &SessionId,
    ) -> Option<super::super::configuration::settings::EffectiveConfiguration> {
        let application = self.applications.lock();
        let mut view =
            crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
                &self.configuration_runtime(id)?,
            )?;
        view.application = application.view(id.as_ref());
        view.process_bindings = Some(self.process_policy());
        Some(view)
    }

    /// Process-local automatic source repair observations (Issue #464).
    pub(crate) fn source_repairs(&self) -> &super::super::configuration::repair::RepairLedger {
        self.configuration.repairs()
    }

    pub(crate) fn configuration_changes(&self) -> watch::Receiver<u64> {
        self.applications.subscribe()
    }

    pub(crate) fn next_configuration_application(
        &self,
        versions: &std::collections::BTreeMap<String, u64>,
    ) -> Option<super::super::configuration::application::ConfigurationApplication> {
        self.applications.lock().next_view(versions)
    }

    pub(crate) fn adopt_configuration(
        &self,
        session: &SessionId,
        identity: &super::super::configuration::application::ApplicationIdentity,
        expected_binding: u64,
    ) -> Result<
        super::super::configuration::application::ConfigurationApplication,
        crate::runtime::adoption::AdoptionError,
    > {
        self.applications
            .adopt(self, session, identity, expected_binding)
    }

    pub(crate) async fn reconcile_configuration(
        &self,
        target: &super::super::configuration::settings::SourceTarget,
    ) -> Result<
        super::super::configuration::application::ConfigurationApplication,
        SourceSettingsError,
    > {
        let owner = self.clone();
        let target = target.clone();
        tokio::spawn(async move {
            owner.coordinate_source(&target).await?;
            owner
                .applications
                .lock()
                .view(&target.application_scope())
                .ok_or(SourceSettingsError::Source(
                    super::super::configuration::settings::SettingsError::Io,
                ))
        })
        .await
        .map_err(|_| {
            SourceSettingsError::Source(
                super::super::configuration::settings::SettingsError::Committed,
            )
        })?
    }

    async fn coordinate_source(
        &self,
        target: &super::super::configuration::settings::SourceTarget,
    ) -> Result<(), SourceSettingsError> {
        let owner = self.clone();
        let target = target.clone();
        tokio::task::spawn_blocking(move || {
            target.validate().map_err(SourceSettingsError::Source)?;
            // Explicit reconciliation observes external edits: the User source
            // composes every target, the Workspace only its own.
            owner.repair_sources(&target);
            let mut application = owner.applications.lock();
            owner.capture_source_consumers(&mut application, &target);
            owner.applications.notify(&application);
            drop(application);
            owner.applications.run(owner.clone());
            Ok(())
        })
        .await
        .map_err(|_| {
            SourceSettingsError::Source(super::super::configuration::settings::SettingsError::Io)
        })?
    }

    /// Automatic `rustx.toml` repair at a native source boundary (Issue #464).
    ///
    /// Each source is its own transaction, serialized with source writes and
    /// coordinator capture by the application lock. A committed repair
    /// transfers to the coordinator exactly like a committed source write, so
    /// one commit is captured once; a healthy or already-repaired source takes
    /// no lock on disk, writes nothing and captures nothing.
    ///
    /// The scope is admitted once, before either source: a selected Workspace
    /// that is not isolated from the runtime root repairs neither the User nor
    /// the Workspace, and the fresh resolution that follows reports it. Once
    /// admitted, the sources stay independent transactions.
    pub(in crate::local_runtime) fn repair_sources(
        &self,
        scope: &super::super::configuration::settings::SourceTarget,
    ) {
        use super::super::configuration::repair::RepairOutcome;
        use super::super::configuration::settings::SourceTarget;
        let Ok(scope) = self.configuration.admit_repair_scope(scope) else {
            return;
        };
        // User first: the Workspace composes over the User it will be
        // resolved with.
        let workspace = scope.target().workspace().map(|_| scope.target().clone());
        let targets = std::iter::once(SourceTarget::User).chain(workspace);
        for target in targets {
            let mut application = self.applications.lock();
            let outcome = self.configuration.repair_source(&target, &scope);
            if outcome == RepairOutcome::Committed {
                self.capture_source_consumers(&mut application, &target);
                self.applications.notify(&application);
            }
            drop(application);
            if outcome == RepairOutcome::Committed {
                self.applications.run(self.clone());
            }
        }
    }

    /// The bound User and the exact selected Workspace of one Session, before
    /// its fresh resolution. No ancestor or other Workspace is consulted. The
    /// Workspace is located exactly as that resolution locates it; a request
    /// whose locations it rejects selects no Workspace and repairs nothing,
    /// never the User alone.
    pub(super) fn repair_session_sources(
        &self,
        input: &super::super::configuration::SessionConfigInput,
    ) {
        let Ok((locations, _)) = self.configuration.resolve_locations(input) else {
            return;
        };
        self.repair_sources(
            &super::super::configuration::settings::SourceTarget::Workspace {
                directory: locations.workspace,
            },
        );
    }

    pub(super) fn capture_source_consumers(
        &self,
        application: &mut super::super::configuration::application::ApplicationState,
        target: &super::super::configuration::settings::SourceTarget,
    ) {
        use super::super::configuration::settings::SourceTarget;
        let captured = self.configuration.capture_source_settings(target);
        let revision = captured.as_ref().ok().map(|(_, revision)| revision.clone());
        let process = captured
            .as_ref()
            .ok()
            .and_then(|(settings, _)| settings.user.authored.as_ref())
            .map(|document| document.app_server.clone().unwrap_or_default())
            .ok_or_else(|| "User process policy source is invalid or unreadable".into())
            .and_then(|policy| policy.validate().map(|()| policy));
        application.capture_source(target, revision, process);
        if let SourceTarget::Workspace { directory } = target {
            let input = super::super::configuration::SessionConfigInput::new(directory.clone());
            application.record_source(directory, &self.configuration.capture_application(&input));
        }
        if matches!(target, SourceTarget::User) {
            // A User commit also refreshes the desired capture of every
            // Workspace identity with retained native authority, including
            // those without any live Session. This never touches `available`;
            // only successful fenced preparation publishes there.
            for directory in application.known_source_directories() {
                let input = super::super::configuration::SessionConfigInput::new(directory.clone());
                application
                    .record_source(&directory, &self.configuration.capture_application(&input));
            }
        }
        let inputs: Vec<_> = self
            .sessions
            .configuration_bindings
            .lock()
            .expect("Session configuration bindings")
            .iter()
            .filter(|(_, binding)| {
                target
                    .workspace()
                    .is_none_or(|directory| directory == binding.input.cwd)
            })
            .map(|(session, binding)| (session.clone(), binding.input.clone()))
            .collect();
        for (session, input) in inputs {
            application.capture(
                session.to_string(),
                &input.cwd,
                self.configuration.capture_session_application(&input),
            );
        }
    }

    /// Source authoring never loads, creates, or derives authority from a Session.
    /// Writes transfer to a native-owned task before persistence begins. Source
    /// publication and coordinator capture are serialized by the application lock.
    /// The task survives destruction of the RPC waiter, including after commit.
    /// # Errors
    /// Returns typed source validation, CAS, I/O, or uncertain-commit errors.
    pub async fn source_settings(
        &self,
        target: &super::super::configuration::settings::SourceTarget,
    ) -> Result<super::super::configuration::settings::SourceSettings, SourceSettingsError> {
        let owner = self.clone();
        let target = target.clone();
        tokio::task::spawn_blocking(move || {
            target.validate().map_err(SourceSettingsError::Source)?;
            let application = owner.applications.lock();
            let result = owner.configuration.read_source_settings(&target);
            let mut projection = result;
            if let Ok(projection) = &mut projection {
                projection.application = application.view(&target.application_scope());
                projection.process_bindings = Some(owner.process_policy());
                projection.session_models = target.workspace().map(|directory| {
                    use super::super::configuration::settings::SessionModelsView;
                    match application.session_creation_models(
                        &owner.configuration,
                        directory,
                        &owner.credentials,
                    ) {
                        Ok((catalog, default_model)) => SessionModelsView::Available {
                            catalog,
                            default_model: Box::new(default_model),
                        },
                        Err(diagnostic) => SessionModelsView::Unavailable { diagnostic },
                    }
                });
            }
            drop(application);
            projection.map_err(SourceSettingsError::Source)
        })
        .await
        .map_err(|_| {
            SourceSettingsError::Source(
                super::super::configuration::settings::SettingsError::Committed,
            )
        })?
    }

    /// The Workflow files physically owned by one source scope. A pure read:
    /// it takes no application lock and publishes nothing.
    /// # Errors
    /// Rejects a redirected target.
    pub async fn workflow_library(
        &self,
        target: super::super::configuration::settings::SourceTarget,
    ) -> Result<super::super::workflow_authoring::WorkflowLibrary, SourceSettingsError> {
        let owner = self.clone();
        Self::blocking(move || owner.configuration.workflow_library(&target)).await
    }

    /// The complete authorized source of one exact Workflow file.
    /// # Errors
    /// Rejects a redirected target and an unreadable file.
    pub async fn workflow_source(
        &self,
        target: super::super::configuration::settings::SourceTarget,
        id: crate::runtime::workflow::WorkflowId,
    ) -> Result<super::super::workflow_authoring::WorkflowSource, SourceSettingsError> {
        let owner = self.clone();
        Self::blocking(move || owner.configuration.workflow_source(&target, &id)).await
    }

    /// Read one bounded page of registered Workflow Model/Profile choices.
    /// # Errors
    /// Rejects redirected source ownership or a changed catalog revision.
    pub async fn workflow_model_choices(
        &self,
        target: super::super::configuration::settings::SourceTarget,
        query: super::super::workflow_authoring::WorkflowModelChoicesQuery,
        expected_revision: Option<String>,
    ) -> Result<super::super::workflow_authoring::WorkflowModelChoices, SourceSettingsError> {
        let owner = self.clone();
        Self::blocking(move || {
            owner
                .configuration
                .workflow_model_choices(&target, query, expected_revision.as_deref())
        })
        .await
    }

    /// Check one candidate Workflow document offline.
    /// # Errors
    /// Rejects a redirected target or context.
    pub async fn check_workflow(
        &self,
        target: super::super::configuration::settings::SourceTarget,
        id: crate::runtime::workflow::WorkflowId,
        document: crate::runtime::workflow::WorkflowDefinition,
        context: Option<std::path::PathBuf>,
        explain: bool,
    ) -> Result<super::super::workflow_authoring::WorkflowCheckReport, SourceSettingsError> {
        let owner = self.clone();
        Self::blocking(move || {
            owner
                .configuration
                .check_workflow(&target, &id, document, context.as_deref(), explain)
        })
        .await
    }

    async fn blocking<T: Send + 'static>(
        read: impl FnOnce() -> Result<T, super::super::configuration::settings::SettingsError>
        + Send
        + 'static,
    ) -> Result<T, SourceSettingsError> {
        tokio::task::spawn_blocking(read)
            .await
            .map_err(|_| {
                SourceSettingsError::Source(
                    super::super::configuration::settings::SettingsError::Io,
                )
            })?
            .map_err(SourceSettingsError::Source)
    }

    /// Commit one source edit and transfer its application responsibility.
    /// The revision is captured from candidate bytes at native publication,
    /// never from a subsequent current-state read.
    /// # Errors
    /// Stale/invalid sources and unconfirmed durable commitment remain explicit.
    pub async fn write_source_settings(
        &self,
        target: &super::super::configuration::settings::SourceTarget,
        expected: String,
        mutation: super::super::configuration::settings::SourceMutation,
    ) -> Result<super::super::configuration::observation::SourceWriteReceipt, SourceSettingsError>
    {
        let owner = self.clone();
        let target = target.clone();
        tokio::task::spawn_blocking(move || {
            target.validate().map_err(SourceSettingsError::Source)?;
            let mut application = owner.applications.lock();
            let result = owner
                .configuration
                .write_source_settings(&target, &expected, mutation);
            if result.is_ok()
                || matches!(
                    &result,
                    Err(super::super::configuration::settings::SettingsError::Committed)
                )
            {
                owner.capture_source_consumers(&mut application, &target);
                owner.applications.notify(&application);
            }
            let configuration_revision = application
                .view(&target.application_scope())
                .map(|application| application.version);
            drop(application);
            owner.applications.run(owner.clone());
            #[cfg(test)]
            owner
                .configuration
                .test_hooks
                .reach("after_coordination_transfer");
            result
                .map(
                    |revision| super::super::configuration::observation::SourceWriteReceipt {
                        revision,
                        configuration_revision,
                    },
                )
                .map_err(SourceSettingsError::Source)
        })
        .await
        .map_err(|_| {
            SourceSettingsError::Source(
                super::super::configuration::settings::SettingsError::Committed,
            )
        })?
    }
}

#[derive(Debug)]
pub enum SourceSettingsError {
    Source(super::super::configuration::settings::SettingsError),
}
