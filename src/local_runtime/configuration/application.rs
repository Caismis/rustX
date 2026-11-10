//! Native application identity and publication serialization.
//!
//! This owner serializes source ingestion and final publication, not execution
//! admission or resource preparation. A single worker drains the latest desired
//! attempt; replacing pending work never makes stale work publishable again.
//! A prepared candidate is staged in its Session's runtime, which commits it
//! automatically at its own safe boundary; no client confirms it.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

mod worker;
use crate::runtime::configuration_commit::{ConfigurationChangeReason, DesiredApplication};

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ApplyUnit {
    ExecutionPolicy,
    Capabilities,
    Instructions,
    Provider,
    SharedCapacity,
    ProcessBindings,
}

#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum UnitApplication {
    Applied,
    Preparing,
    /// Prepared and staged; the runtime commits it automatically once the
    /// execution that owns the current configuration settles.
    Ready,
    /// The previous configuration of this unit remains in use.
    Failed {
        diagnostic: String,
    },
    /// The resources were published but the commit could not be recorded;
    /// native recovery reconciles from durable authority.
    Uncertain {
        diagnostic: String,
    },
    ProcessRestart,
}

/// Source attempt identity is independent of both content and execution version.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct ApplicationIdentity {
    /// None means immutable input capture failed; no source revision is fabricated.
    pub input_revision: Option<String>,
    pub attempt: u64,
}

#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct ConfigurationApplication {
    /// The application-scope key. A Session application carries the Session
    /// identity here; a source application carries `SourceTarget::
    /// application_scope`. It is an application key, never a source owner.
    pub scope: String,
    /// The authored source owners this application composes, lowest authority
    /// first. The User document always participates; a Workspace-rooted capture
    /// also names the exact canonical configuration directory it was taken
    /// from. This is the only fact that answers which authoring surface owns a
    /// configuration failure; it is never derived from `scope`.
    pub sources: Vec<super::settings::SourceTarget>,
    pub version: u64,
    pub desired: ApplicationIdentity,
    pub units: BTreeMap<ApplyUnit, UnitApplication>,
}

pub(crate) enum ApplicationRead {
    Busy,
    Ready(Option<ConfigurationApplication>),
}

#[derive(Clone, Debug)]
pub(crate) struct ConfigurationApplications {
    inner: Arc<Mutex<ApplicationState>>,
    changed: tokio::sync::watch::Sender<u64>,
}

#[derive(Default)]
pub(crate) struct ApplicationState {
    // Source authority survives Session retirement and runtime eviction. Keys
    // are canonical execution/configuration directories within this User owner.
    available: BTreeMap<std::path::PathBuf, super::ProspectiveSessionConfig>,
    desired_sources: BTreeMap<std::path::PathBuf, Result<CapturedApplication, String>>,
    scope_sources: BTreeMap<String, std::path::PathBuf>,
    // The exact authored source target each application scope was captured
    // from. A Session scope is keyed by its Session identity but owned by the
    // Workspace source at its configuration directory; an application scope key
    // is never itself a source owner.
    scope_targets: BTreeMap<String, super::settings::SourceTarget>,
    deferred: BTreeSet<String>,
    desired_process: Option<super::super::app_server_policy::AppServerPolicy>,
    process: Option<(
        super::super::app_server_policy::AppServerPolicy,
        UnitApplication,
    )>,
    next_attempt: u64,
    version: u64,
    scopes: BTreeMap<String, ConfigurationApplication>,
    pending: BTreeMap<String, ApplicationIdentity>,
    inputs: BTreeMap<String, Result<CapturedApplication, String>>,
    source_inputs:
        BTreeMap<String, Result<super::super::app_server_policy::AppServerPolicy, String>>,
    worker_running: bool,
    #[cfg(test)]
    worker_starts: usize,
    // The latest desired attempt per scope, shared with the candidate a
    // runtime holds staged so a superseded candidate can never commit.
    fences: BTreeMap<String, Arc<AtomicU64>>,
    reasons: BTreeMap<String, ConfigurationChangeReason>,
    // The runtime allocation each scope's Ready candidate is staged in, with
    // the application that staged it; only that application's settlement
    // clears it.
    pub(crate) staged_in: BTreeMap<String, (Arc<()>, ApplicationIdentity)>,
    // The native authority of each scope's attached runtime allocation over
    // unrecorded commits. Views read it directly, so an application never
    // reports success over a commit that allocation could not record.
    health: BTreeMap<String, crate::runtime::conversation_runtime::ConfigurationHealth>,
}

#[cfg(test)]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SelectionApplicationProbe {
    version: u64,
    next_attempt: u64,
    worker_starts: usize,
    pending: BTreeMap<String, ApplicationIdentity>,
}

#[derive(Clone)]
pub(crate) struct CapturedApplication {
    pub(crate) policy: super::IndependentPolicy,
    pub(crate) process: super::super::app_server_policy::AppServerPolicy,
    pub(crate) revision: String,
    pub(crate) context: Result<super::ProspectiveSessionConfig, String>,
    /// Every file the capture read or tried to read, whether or not
    /// `context` succeeded: a newly selected instruction file that is not yet
    /// valid stays an input, so a fix to it alone is observed. Diagnostics
    /// keep naming the authored file at fault.
    pub(crate) read: Vec<std::path::PathBuf>,
}

impl std::fmt::Debug for ApplicationState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApplicationState")
            .field("scopes", &self.scopes)
            .finish_non_exhaustive()
    }
}

impl Default for ConfigurationApplications {
    fn default() -> Self {
        Self {
            inner: Arc::default(),
            changed: tokio::sync::watch::channel(0).0,
        }
    }
}

impl ConfigurationApplications {
    #[cfg(test)]
    pub(crate) fn is_deferred(&self, scope: &str) -> bool {
        self.lock().deferred.contains(scope)
    }

    /// The state a superseded candidate leaves when its allocation unloads
    /// before the successor desire reaches a runtime: its marker names a
    /// replaced allocation and the successor deferred cold.
    #[cfg(test)]
    pub(crate) fn strand_in_replaced_allocation(&self, scope: &str) {
        let mut state = self.lock();
        if let Some((owner, _)) = state.staged_in.get_mut(scope) {
            *owner = Arc::new(());
        }
        state.deferred.insert(scope.to_owned());
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, ApplicationState> {
        self.inner.lock().expect("configuration application owner")
    }

    pub(crate) fn try_lock(&self) -> Option<MutexGuard<'_, ApplicationState>> {
        match self.inner.try_lock() {
            Ok(state) => Some(state),
            Err(std::sync::TryLockError::WouldBlock) => None,
            Err(std::sync::TryLockError::Poisoned(_)) => {
                panic!("configuration application owner")
            }
        }
    }

    /// Attach `runtime` to its scope's application when `current` confirms,
    /// under this lock, that it is still the scope's resident: a replaced
    /// allocation's late rebind never takes the application back.
    pub(crate) fn rebind_runtime(
        &self,
        scope: &str,
        runtime: &crate::runtime::conversation_runtime::ConversationRuntime,
        current: impl FnOnce() -> bool,
    ) -> bool {
        let mut state = self.lock();
        if !current() {
            return false;
        }
        let attached = state.attach(scope, runtime);
        // A candidate staged in a replaced runtime can never commit into its
        // successor; the current input is prepared again for this allocation.
        // Attaching the allocation that already holds it changes nothing.
        let obsolete = state
            .staged_in
            .get(scope)
            .is_some_and(|(owner, _)| !Arc::ptr_eq(owner, runtime.configuration_owner()));
        if obsolete {
            state.staged_in.remove(scope);
        }
        // This rebind consumes a cold deferral whether or not the staged
        // candidate is obsolete, so a later attach to the same allocation
        // finds nothing to prepare again.
        let deferred = state.deferred.remove(scope);
        if !obsolete && !deferred {
            // A new allocation changes the views even with nothing to prepare.
            if attached {
                self.notify(&state);
            }
            return false;
        }
        state
            .reasons
            .insert(scope.to_owned(), ConfigurationChangeReason::Reallocation);
        let mut input = state
            .scope_sources
            .get(scope)
            .and_then(|source| state.desired_sources.get(source))
            .cloned()
            .unwrap_or_else(|| state.captured_input(scope));
        if let Ok(captured) = &mut input
            && let Ok(context) = &mut captured.context
        {
            context.input.model = Some(runtime.model_view().configured);
        }
        state.capture_binding(scope.to_owned(), input);
        self.notify(&state);
        true
    }

    pub(crate) fn forget(&self, scope: &str) {
        let mut state = self.lock();
        state.scopes.remove(scope);
        state.scope_sources.remove(scope);
        state.scope_targets.remove(scope);
        state.pending.remove(scope);
        state.inputs.remove(scope);
        state.deferred.remove(scope);
        state.reasons.remove(scope);
        state.staged_in.remove(scope);
        state.health.remove(scope);
        if let Some(fence) = state.fences.remove(scope) {
            fence.store(0, Ordering::SeqCst);
        }
        state.version = state
            .version
            .checked_add(1)
            .expect("application version exhausted");
        self.notify(&state);
    }

    pub(crate) fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changed.subscribe()
    }

    pub(crate) fn notify(&self, state: &ApplicationState) {
        self.changed.send_replace(state.version);
    }
}

impl ApplicationState {
    /// Runtime catalog publication changes the native baseline, not authored
    /// intent. Reprepare the latest captured input, including its model choice.
    pub(crate) fn rebase_runtime_catalog(&mut self, scope: &str) -> bool {
        // A staged candidate prepared against an older binding is superseded
        // by the runtime itself at its commit point; only in-flight
        // preparation must be redone against the published catalog.
        let needs_rebase = self.scopes.get(scope).is_some_and(|application| {
            [
                ApplyUnit::Capabilities,
                ApplyUnit::Instructions,
                ApplyUnit::Provider,
            ]
            .iter()
            .any(|unit| {
                matches!(
                    application.units.get(unit),
                    Some(UnitApplication::Preparing)
                )
            })
        });
        if !needs_rebase {
            return false;
        }
        let Some(input) = self.inputs.get(scope).cloned() else {
            return false;
        };
        self.capture_binding(scope.to_owned(), input);
        true
    }

    #[cfg(test)]
    pub(crate) fn selection_probe(&self) -> SelectionApplicationProbe {
        SelectionApplicationProbe {
            version: self.version,
            next_attempt: self.next_attempt,
            worker_starts: self.worker_starts,
            pending: self.pending.clone(),
        }
    }

    /// Initial resolution happens only for an uninitialized source scope. Once
    /// established, authored input cannot bypass successful native publication.
    /// A new Session's selection must resolve. A `persisted` Session's
    /// selection is its own earlier choice: when the catalog no longer admits
    /// it, the Session still binds and loads, reporting it unavailable.
    pub(crate) fn initial_binding(
        &mut self,
        configuration: &super::UserConfigManager,
        input: &super::SessionConfigInput,
        credentials: &crate::credentials::CredentialSnapshot,
        persisted: bool,
    ) -> Result<super::AdmittedSessionConfig, String> {
        let key = super::capture::canonical_directory(&input.cwd)?;
        let mut capture = match self.creation_capture(configuration, &key, credentials)? {
            std::borrow::Cow::Borrowed(capture) => capture.clone(),
            std::borrow::Cow::Owned(capture) => {
                self.available.insert(key.clone(), capture.clone());
                capture
            }
        };
        capture.input.model = Some(
            input
                .model
                .clone()
                .or_else(|| capture.config.agent.model.clone())
                .ok_or(NO_CONVERSATION_MODEL)?,
        );
        Self::validate_selection(&capture, credentials, persisted)?;
        capture.admit(|| credentials.clone())
    }

    /// The capture a Session created in the canonical directory `key` binds:
    /// its published source, or on first use a freshly resolved and validated
    /// one. Nothing is published here; `initial_binding` owns that step.
    fn creation_capture(
        &self,
        configuration: &super::UserConfigManager,
        key: &std::path::Path,
        credentials: &crate::credentials::CredentialSnapshot,
    ) -> Result<std::borrow::Cow<'_, super::ProspectiveSessionConfig>, String> {
        if let Some(capture) = self.available.get(key) {
            // A first usable model can bootstrap creation after an empty
            // catalog. Preserve the Workspace's last published resources.
            if capture.config.agent.model.is_none()
                && let Some(desired) = self
                    .desired_sources
                    .get(key)
                    .and_then(|input| input.as_ref().ok())
                    .and_then(|input| input.context.as_ref().ok())
                && desired.config.agent.model.is_some()
                && let Some(candidate) =
                    Self::creation_catalog_candidate(capture, desired, credentials, false)
            {
                return Ok(std::borrow::Cow::Owned(candidate));
            }
            return Ok(std::borrow::Cow::Borrowed(capture));
        }
        let capture = configuration
            .capture_application(
                &super::SessionConfigInput::new(key.to_path_buf()),
                Some(credentials),
            )?
            .context?;
        Self::validate_default(&capture, credentials)?;
        capture.validate_resource_authority()?;
        Ok(std::borrow::Cow::Owned(capture))
    }

    /// The exact catalog `session/models` serves for a Session created in
    /// `cwd` now. Read-only: it neither publishes a capture nor creates a
    /// Session, and it fails where Session creation would fail.
    pub(crate) fn session_creation_models(
        &self,
        configuration: &super::UserConfigManager,
        cwd: &std::path::Path,
        credentials: &crate::credentials::CredentialSnapshot,
    ) -> Result<
        (
            crate::model::catalog::ModelCatalogView,
            crate::model::session::SessionModelConfig,
        ),
        String,
    > {
        let key = super::capture::canonical_directory(cwd)?;
        let capture = self.creation_capture(configuration, &key, credentials)?;
        let default = capture
            .config
            .agent
            .model
            .clone()
            .ok_or(NO_CONVERSATION_MODEL)?;
        let catalog = capture
            .models
            .resolve(credentials)
            .map_err(|error| error.to_string())?;
        Ok((
            crate::model::invocation::ModelBindingRegistry::new(catalog)
                .map_err(|error| error.to_string())?
                .catalog_view(),
            default,
        ))
    }

    /// Join the captured desired source without rereading authored files. The
    /// caller publishes the retained binding under this same coordinator lock.
    /// Allocation work waits for natural residency, including when creation
    /// straddles source preparation/publication.
    pub(crate) fn register_session_scope(
        &mut self,
        scope: String,
        adopted: &super::AdmittedSessionConfig,
    ) {
        let source = adopted.input.cwd.clone();
        self.scope_sources.insert(scope.clone(), source.clone());
        self.own_scope(&scope, &source);
        if let Some(mut input) = self.desired_sources.get(&source).cloned() {
            if let Ok(captured) = &mut input
                && let Ok(context) = &mut captured.context
            {
                context.input.model = Some(adopted.session_model().clone());
            }
            // Membership is unconditional; only unresolved Session-owned units
            // require an application. Failed capture remains unresolved, while
            // process restart alone never creates allocation work.
            // Session-effective equality settles only this joining Session. A
            // desired source generation that never published still owes this
            // Workspace native publication, so that work rides this scope.
            if let Ok(captured) = &input
                && let Ok(context) = &captured.context
                && context.same_capabilities(adopted)
                && context.same_skill_catalog(adopted)
                && context.same_context(adopted)
                && context.same_provider(adopted)
                && captured.policy.approval_mode == adopted.config.approval_mode
                && captured.policy.model_timeout_policy == adopted.config.model_timeout_policy
                && captured.policy.tool_deadline_policy == adopted.config.tool_deadline_policy
                && captured.policy.mcp_startup_policy == adopted.config.mcp_startup_policy
                && captured.policy.subagents == adopted.config.subagents
                && self
                    .available
                    .get(&source)
                    .is_some_and(|available| available.source_revisions == context.source_revisions)
            {
                return;
            }
            self.capture_binding(scope.clone(), input);
            self.pending.remove(&scope);
            self.deferred.insert(scope);
        }
    }

    fn validate_selection(
        capture: &super::ProspectiveSessionConfig,
        credentials: &crate::credentials::CredentialSnapshot,
        persisted: bool,
    ) -> Result<(), String> {
        capture
            .config
            .validate_resources()
            .map_err(|error| error.to_string())?;
        for name in &capture.admitted_agent_dependencies() {
            if capture.subagents.get(name).is_none() {
                return Err(format!("unknown Agent {name}"));
            }
        }
        let catalog = capture
            .models
            .resolve(credentials)
            .map_err(|error| error.to_string())?;
        let models = crate::model::invocation::ModelBindingRegistry::new(catalog)
            .map_err(|error| error.to_string())?;
        let selection = capture.session_model().clone();
        let model = if persisted {
            crate::model::session::SessionModelState::restore(models, selection)
        } else {
            crate::model::session::SessionModelState::new(models, selection)
        }
        .map_err(|error| error.to_string())?;
        // The same native budget validator used at runtime composition.
        crate::runtime::conversation_runtime::validate_model_context(
            &capture.config.context_policy(),
            &model,
        )
        .map_err(|error| error.message)
    }

    fn validate_default(
        capture: &super::ProspectiveSessionConfig,
        credentials: &crate::credentials::CredentialSnapshot,
    ) -> Result<(), String> {
        if capture.config.agent.model.is_none() {
            return Ok(());
        }
        let mut default = capture.clone();
        default.input.model = None;
        Self::validate_selection(&default, credentials, false)
    }

    fn make_available(
        &mut self,
        scope: &str,
        identity: &ApplicationIdentity,
        capture: &super::ProspectiveSessionConfig,
        credentials: &crate::credentials::CredentialSnapshot,
    ) -> Result<(), String> {
        Self::validate_default(capture, credentials)?;
        capture.validate_resource_authority()?;
        if self.current(scope, identity)
            && self
                .desired_sources
                .get(&capture.input.cwd)
                .and_then(|input| input.as_ref().ok())
                .is_some_and(|input| Some(&input.revision) == identity.input_revision.as_ref())
        {
            let mut available = capture.clone();
            available.input.model = None;
            self.available
                .insert(available.input.cwd.clone(), available);
        }
        Ok(())
    }

    /// Called while the native source commit lock is held. Retry deliberately
    /// allocates a new identity even when the input digest did not change.
    pub(crate) fn desire(
        &mut self,
        scope: String,
        input_revision: Option<String>,
    ) -> ApplicationIdentity {
        self.next_attempt = self
            .next_attempt
            .checked_add(1)
            .expect("application identity exhausted");
        self.version = self
            .version
            .checked_add(1)
            .expect("application version exhausted");
        let identity = ApplicationIdentity {
            input_revision,
            attempt: self.next_attempt,
        };
        let sources = self.source_owners(&scope);
        self.scopes.insert(
            scope.clone(),
            ConfigurationApplication {
                scope: scope.clone(),
                sources,
                version: self.version,
                desired: identity.clone(),
                units: BTreeMap::from([
                    (ApplyUnit::ExecutionPolicy, UnitApplication::Preparing),
                    (ApplyUnit::Capabilities, UnitApplication::Preparing),
                    (ApplyUnit::Instructions, UnitApplication::Preparing),
                    (ApplyUnit::Provider, UnitApplication::Preparing),
                    (ApplyUnit::SharedCapacity, UnitApplication::Preparing),
                    (ApplyUnit::ProcessBindings, UnitApplication::Preparing),
                ]),
            },
        );
        self.fences
            .entry(scope.clone())
            .or_default()
            .store(identity.attempt, Ordering::SeqCst);
        self.pending.insert(scope, identity.clone());
        identity
    }

    /// The currency fence a staged candidate of `identity` carries.
    pub(crate) fn desired(
        &mut self,
        scope: &str,
        identity: &ApplicationIdentity,
    ) -> DesiredApplication {
        DesiredApplication::new(
            self.fences.entry(scope.to_owned()).or_default().clone(),
            identity.attempt,
        )
    }

    /// Why the scope's current application exists.
    pub(crate) fn reason(&self, scope: &str) -> ConfigurationChangeReason {
        self.reasons
            .get(scope)
            .copied()
            .unwrap_or(ConfigurationChangeReason::Source)
    }

    /// Ingestion is part of the source transaction. The worker receives these
    /// exact bytes; it never completes a candidate by reading newer files.
    pub(crate) fn capture(
        &mut self,
        scope: String,
        source: &std::path::Path,
        input: Result<CapturedApplication, String>,
    ) -> ApplicationIdentity {
        self.record_source(source, &input);
        self.reasons
            .insert(scope.clone(), ConfigurationChangeReason::Source);
        self.scope_sources
            .insert(scope.clone(), source.to_path_buf());
        self.own_scope(&scope, source);
        if let Ok(capture) = &input
            && let Some(current) = self.scopes.get(&scope)
            && current.desired.input_revision.as_ref() == Some(&capture.revision)
            && !current
                .units
                .values()
                .any(|unit| matches!(unit, UnitApplication::Failed { .. }))
        {
            return current.desired.clone();
        }
        self.capture_binding(scope, input)
    }

    /// A different Session model or physical allocation needs a new fenced
    /// application even when its source manifest is unchanged.
    pub(crate) fn capture_binding(
        &mut self,
        scope: String,
        input: Result<CapturedApplication, String>,
    ) -> ApplicationIdentity {
        let revision = input.as_ref().ok().map(|capture| capture.revision.clone());
        let identity = self.desire(scope.clone(), revision);
        self.inputs.insert(scope, input);
        identity
    }

    /// Reconsider an existing captured source after explicit Session intent
    /// changes. Selection never ingests source files or adopts pending context.
    pub(crate) fn selection_changed(
        &mut self,
        scope: &str,
        selection: crate::model::session::SessionModelConfig,
    ) {
        // A Session with no source application has nothing to reconsider.
        // Selection must not manufacture a failed application or ingest files
        // merely to populate one.
        let Some(mut input) = self.inputs.get(scope).cloned() else {
            return;
        };
        if let Ok(captured) = &mut input
            && let Ok(context) = &mut captured.context
        {
            context.input.model = Some(selection);
        }
        self.reasons
            .insert(scope.to_owned(), ConfigurationChangeReason::ModelSelection);
        self.capture_binding(scope.to_owned(), input);
    }

    pub(crate) fn capture_source(
        &mut self,
        target: &super::settings::SourceTarget,
        revision: Option<String>,
        process: Result<super::super::app_server_policy::AppServerPolicy, String>,
    ) {
        let scope = target.application_scope();
        // A source application scope owns exactly the target it was captured
        // for. The User source scope has no Workspace owner at all.
        match target {
            super::settings::SourceTarget::User => {
                self.scope_targets.remove(&scope);
            }
            super::settings::SourceTarget::Workspace { directory } => {
                self.own_scope(&scope, directory);
            }
        }
        if self
            .scopes
            .get(&scope)
            .is_some_and(|view| view.desired.input_revision == revision)
            && self.source_inputs.get(&scope) == Some(&process)
            && self.scopes.get(&scope).is_some_and(|view| {
                !view
                    .units
                    .values()
                    .any(|unit| matches!(unit, UnitApplication::Failed { .. }))
            })
        {
            return;
        }
        self.desire(scope.clone(), revision);
        self.scopes
            .get_mut(&scope)
            .expect("registered source")
            .units
            .retain(|unit, _| *unit == ApplyUnit::ProcessBindings);
        if let Ok(policy) = &process {
            self.desired_process = Some(policy.clone());
        }
        self.source_inputs.insert(scope, process);
    }

    pub(crate) fn record_source(
        &mut self,
        source: &std::path::Path,
        input: &Result<CapturedApplication, String>,
    ) {
        self.desired_sources
            .insert(source.to_path_buf(), input.clone());
        if let Ok(input) = input {
            self.desired_process = Some(input.process.clone());
        }
    }

    /// A typed model-catalog edit reaches new Sessions at the source commit,
    /// even when no Session worker exists. Other units keep their previous
    /// creation authority. Existing Sessions still apply the ordinary capture.
    pub(crate) fn publish_creation_catalog(
        &mut self,
        workspace: Option<&std::path::Path>,
        credentials: &crate::credentials::CredentialSnapshot,
        removing: bool,
    ) {
        for (directory, available) in &mut self.available {
            if workspace.is_some_and(|workspace| workspace != directory) {
                continue;
            }
            let Some(desired) = self
                .desired_sources
                .get(directory)
                .and_then(|input| input.as_ref().ok())
                .and_then(|input| input.context.as_ref().ok())
            else {
                continue;
            };
            if let Some(candidate) =
                Self::creation_catalog_candidate(available, desired, credentials, removing)
            {
                *available = candidate;
            }
        }
    }

    fn creation_catalog_candidate(
        available: &super::ProspectiveSessionConfig,
        desired: &super::ProspectiveSessionConfig,
        credentials: &crate::credentials::CredentialSnapshot,
        removing: bool,
    ) -> Option<super::ProspectiveSessionConfig> {
        if desired.same_catalog(available)
            || !desired.preserves_adopted_models(available, |model| {
                available.models.model(model).is_err()
                    || (removing && desired.models.model(model).is_err())
            })
        {
            return None;
        }
        let candidate = available.clone().with_creation_catalog(desired);
        Self::validate_default(&candidate, credentials).ok()?;
        candidate.validate_resource_authority().ok()?;
        Some(candidate)
    }

    /// Every directory with retained source authority, whether it published a
    /// last-good configuration or only holds a captured desired source.
    pub(crate) fn known_source_directories(&self) -> Vec<std::path::PathBuf> {
        self.available
            .keys()
            .chain(self.desired_sources.keys())
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// Every Workspace directory an external edit can affect: the retained
    /// source authority plus the configuration directory of each Session
    /// application scope.
    pub(crate) fn watched_source_directories(&self) -> Vec<std::path::PathBuf> {
        self.available
            .keys()
            .chain(self.desired_sources.keys())
            .chain(self.scope_sources.values())
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// Every file a known Workspace's retained capture read, with that
    /// Workspace: the bounded inputs its configuration depends on, such as
    /// selected instruction files outside its fixed sources. A desired
    /// capture also contributes every file it read or tried to read, so one
    /// whose context failed still names the file it failed at.
    pub(crate) fn source_inputs(&self) -> Vec<(std::path::PathBuf, std::path::PathBuf)> {
        let captured = self.available.iter().map(|(directory, config)| {
            (
                directory,
                config.source_revisions.keys().collect::<Vec<_>>(),
            )
        });
        let desired = self
            .desired_sources
            .iter()
            .filter_map(|(directory, capture)| {
                let capture = capture.as_ref().ok()?;
                let mut files: Vec<_> = capture.read.iter().collect();
                if let Ok(config) = &capture.context {
                    files.extend(config.source_revisions.keys());
                }
                Some((directory, files))
            });
        captured
            .chain(desired)
            .flat_map(|(directory, files)| {
                files
                    .into_iter()
                    .map(|file| (directory.clone(), file.clone()))
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn test_available(
        &self,
        directory: &std::path::Path,
    ) -> Option<&super::ProspectiveSessionConfig> {
        self.available.get(directory)
    }

    #[cfg(test)]
    pub(crate) fn test_desired_source(
        &self,
        directory: &std::path::Path,
    ) -> Option<&Result<CapturedApplication, String>> {
        self.desired_sources.get(directory)
    }

    fn captured_input(&self, scope: &str) -> Result<CapturedApplication, String> {
        self.inputs
            .get(scope)
            .cloned()
            .unwrap_or_else(|| Err("missing captured inputs".into()))
    }

    /// Whether the worker has exited with nothing left to prepare.
    #[cfg(test)]
    pub(crate) fn worker_idle(&self) -> bool {
        !self.worker_running && self.pending.is_empty()
    }

    pub(crate) fn start_worker(&mut self) -> bool {
        if self.worker_running {
            return false;
        }
        self.worker_running = true;
        #[cfg(test)]
        {
            self.worker_starts += 1;
        }
        true
    }

    pub(crate) fn next(&mut self) -> Option<(String, ApplicationIdentity)> {
        let next = self.pending.pop_first();
        if next.is_none() {
            self.worker_running = false;
        }
        next
    }

    pub(crate) fn current(&self, scope: &str, identity: &ApplicationIdentity) -> bool {
        self.scopes
            .get(scope)
            .is_some_and(|state| state.desired == *identity)
    }

    /// Record which authored source target owns one application scope. Source
    /// ownership is registered at capture, never inferred from the scope key.
    fn own_scope(&mut self, scope: &str, directory: &std::path::Path) {
        self.scope_targets.insert(
            scope.to_owned(),
            super::settings::SourceTarget::Workspace {
                directory: directory.to_path_buf(),
            },
        );
    }

    /// The authored source owners of one application scope, lowest authority
    /// first. Every capture composes the User document; a Workspace-rooted
    /// capture also composes the Workspace document at its exact canonical
    /// configuration directory. No ownership is parsed out of the scope key.
    fn source_owners(&self, scope: &str) -> Vec<super::settings::SourceTarget> {
        use super::settings::SourceTarget;
        match self.scope_targets.get(scope) {
            Some(SourceTarget::Workspace { directory }) => vec![
                SourceTarget::User,
                SourceTarget::Workspace {
                    directory: directory.clone(),
                },
            ],
            _ => vec![SourceTarget::User],
        }
    }

    fn owned(&self, mut view: ConfigurationApplication) -> ConfigurationApplication {
        view.sources = self.source_owners(&view.scope);
        // The attached allocation's own failure, not an application's
        // settlement, decides whether its commits are uncertain.
        if let Some(diagnostic) = self
            .health
            .get(&view.scope)
            .and_then(crate::runtime::conversation_runtime::ConfigurationHealth::unrecorded)
        {
            for unit in view.units.values_mut() {
                if *unit == UnitApplication::Applied {
                    *unit = UnitApplication::Uncertain {
                        diagnostic: diagnostic.clone(),
                    };
                }
            }
        }
        view
    }

    /// Attach the allocation whose authority decides this scope's
    /// uncertainty. A replacement that recorded nothing uncertain recovers
    /// the scope: units its predecessor left uncertain are applied there,
    /// since it runs the generation they published. Returns whether the
    /// allocation is new, which changes the views.
    pub(crate) fn attach(
        &mut self,
        scope: &str,
        runtime: &crate::runtime::conversation_runtime::ConversationRuntime,
    ) -> bool {
        let health = runtime.configuration_health();
        if self
            .health
            .get(scope)
            .is_some_and(|attached| attached.same_allocation(&health))
        {
            return false;
        }
        let healthy = health.unrecorded().is_none();
        self.health.insert(scope.to_owned(), health);
        self.version = self
            .version
            .checked_add(1)
            .expect("application version exhausted");
        if let Some(view) = self.scopes.get_mut(scope) {
            view.version = self.version;
            if healthy {
                for unit in view.units.values_mut() {
                    if matches!(unit, UnitApplication::Uncertain { .. }) {
                        *unit = UnitApplication::Applied;
                    }
                }
            }
        }
        true
    }

    /// A late settlement reported that this scope's allocation could not
    /// record a commit: views read that failure now, so they change.
    pub(crate) fn touch(&mut self, scope: &str) {
        self.version = self
            .version
            .checked_add(1)
            .expect("application version exhausted");
        if let Some(view) = self.scopes.get_mut(scope) {
            view.version = self.version;
        }
    }

    pub(crate) fn next_view(
        &self,
        versions: &BTreeMap<String, u64>,
    ) -> Option<ConfigurationApplication> {
        self.scopes
            .values()
            .find(|view| view.version > versions.get(&view.scope).copied().unwrap_or(0))
            .cloned()
            .map(|view| self.owned(view))
    }

    pub(crate) fn view(&self, scope: &str) -> Option<ConfigurationApplication> {
        self.scopes.get(scope).cloned().map(|view| self.owned(view))
    }

    /// The caller holds this same guard while swapping the complete runtime
    /// pointer. No source commit can fit between this check and that swap.
    pub(crate) fn publish(
        &mut self,
        scope: &str,
        identity: &ApplicationIdentity,
        unit: ApplyUnit,
        commit: impl FnOnce() -> UnitApplication,
    ) -> bool {
        if !self.current(scope, identity) {
            return false;
        }
        let outcome = commit();
        self.version = self
            .version
            .checked_add(1)
            .expect("application version exhausted");
        let state = self.scopes.get_mut(scope).expect("checked scope");
        state.version = self.version;
        state.units.insert(unit, outcome);
        true
    }
}

const NO_CONVERSATION_MODEL: &str = "No model is available. Add or enable a model in Settings.";

#[cfg(test)]
mod tests {
    use super::*;

    /// Issue #391: an application scope key is not a source owner. A Session
    /// application is keyed by its Session identity and must still name the
    /// exact authored documents its capture composed.
    #[test]
    fn s1_application_scope_is_never_the_source_owner() {
        use super::super::settings::SourceTarget;
        let workspace = SourceTarget::Workspace {
            directory: "/workspace/A".into(),
        };
        let mut state = ApplicationState::default();

        // A Session scope: the key is the Session identity, the owners are the
        // User document plus the Workspace document at its capture directory.
        let session = "ses_00000000-0000-7000-8000-000000000001";
        state.capture(
            session.into(),
            std::path::Path::new("/workspace/A"),
            Err("capture failed".into()),
        );
        let view = state.view(session).unwrap();
        assert_eq!(view.scope, session);
        assert!(!view.scope.starts_with("source:"));
        assert_eq!(view.sources, vec![SourceTarget::User, workspace.clone()]);

        // A Workspace source scope owns the same two documents; the User source
        // scope owns only the User document.
        state.capture_source(&workspace, Some("w".into()), Err("policy".into()));
        assert_eq!(
            state.view(&workspace.application_scope()).unwrap().sources,
            vec![SourceTarget::User, workspace.clone()]
        );
        state.capture_source(&SourceTarget::User, Some("u".into()), Err("policy".into()));
        assert_eq!(
            state
                .view(&SourceTarget::User.application_scope())
                .unwrap()
                .sources,
            vec![SourceTarget::User]
        );

        // Every projection carries the owners, and retiring a scope forgets them
        // rather than leaving one Session's directory attached to another key.
        assert!(
            state
                .scopes
                .keys()
                .all(|scope| !state.view(scope).unwrap().sources.is_empty())
        );
        let owner = ConfigurationApplications::default();
        *owner.lock() = state;
        owner.forget(session);
        assert!(owner.lock().view(session).is_none());
        assert!(!owner.lock().scope_targets.contains_key(session));
    }

    #[test]
    fn t08_failed_capture_has_no_manufactured_input_revision() {
        let mut state = ApplicationState::default();
        let identity = state.capture(
            "s".into(),
            std::path::Path::new("/workspace"),
            Err("input changed during capture".into()),
        );
        assert_eq!(identity.input_revision, None);
        assert_eq!(state.view("s").unwrap().desired, identity);
    }

    #[test]
    fn t07_newer_failure_does_not_authorize_old_success_or_failure() {
        let owner = ConfigurationApplications::default();
        let mut state = owner.lock();
        let a = state.desire("s".into(), Some("a".into()));
        let b = state.desire("s".into(), Some("b".into()));
        assert!(state.publish("s", &b, ApplyUnit::Instructions, || {
            UnitApplication::Failed {
                diagnostic: "failed".into(),
            }
        }));
        for outcome in [
            UnitApplication::Applied,
            UnitApplication::Failed {
                diagnostic: "old".into(),
            },
        ] {
            assert!(!state.publish("s", &a, ApplyUnit::Instructions, || outcome));
        }
        assert_eq!(state.view("s").unwrap().desired, b);
    }

    #[test]
    fn t13_same_revision_retry_has_a_new_identity() {
        let mut state = ApplicationState::default();
        let a = state.desire("s".into(), Some("same".into()));
        let b = state.desire("s".into(), Some("same".into()));
        assert_eq!(a.input_revision, b.input_revision);
        assert_ne!(a.attempt, b.attempt);
        assert!(!state.current("s", &a));
    }

    #[test]
    fn t14_pending_work_is_latest_wins_with_one_worker() {
        let mut state = ApplicationState::default();
        assert!(state.start_worker());
        for n in 0..1000 {
            state.desire("s".into(), Some(n.to_string()));
            assert!(!state.start_worker());
            assert_eq!(state.pending.len(), 1);
        }
        assert_eq!(
            state.next().unwrap().1.input_revision.as_deref(),
            Some("999")
        );
        assert!(state.next().is_none());
        assert!(state.start_worker());
    }

    #[test]
    fn t03_units_have_simultaneous_independent_outcomes() {
        let mut state = ApplicationState::default();
        let id = state.desire("s".into(), Some("input".into()));
        for (unit, outcome) in [
            (ApplyUnit::ExecutionPolicy, UnitApplication::Applied),
            (ApplyUnit::Instructions, UnitApplication::Ready),
            (
                ApplyUnit::Capabilities,
                UnitApplication::Failed {
                    diagnostic: "unavailable".into(),
                },
            ),
            (ApplyUnit::ProcessBindings, UnitApplication::ProcessRestart),
        ] {
            assert!(state.publish("s", &id, unit, || outcome));
        }
        let view = state.view("s").unwrap();
        assert_eq!(
            view.units[&ApplyUnit::ExecutionPolicy],
            UnitApplication::Applied
        );
        assert_eq!(
            view.units[&ApplyUnit::ProcessBindings],
            UnitApplication::ProcessRestart
        );
        assert_eq!(view.units[&ApplyUnit::Instructions], UnitApplication::Ready);
        assert!(matches!(
            view.units[&ApplyUnit::Capabilities],
            UnitApplication::Failed { .. }
        ));
    }
}
