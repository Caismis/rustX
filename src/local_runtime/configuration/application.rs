//! Native application identity and publication serialization.
//!
//! This owner serializes source ingestion and final publication, not execution
//! admission or resource preparation. A single worker drains the latest desired
//! attempt; replacing pending work never makes stale work publishable again.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};

mod worker;
use crate::model::request_shape::CacheImpact;

#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AdoptionError {
    Busy,
    NotReady,
    Conflict,
    Failed { diagnostic: String },
}

/// Advisory only; adoption always revalidates the native admission gate.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AdoptionEligibility {
    Eligible,
    Busy,
    Unavailable,
}

pub(crate) struct PreparedConfiguration {
    pub(crate) selection_only: bool,
    pub(crate) owner: Arc<()>,
    pub(crate) baseline: u64,
    pub(crate) preview: Arc<crate::runtime::RuntimeResourceSnapshot>,
    pub(crate) model: crate::model::session::SessionModelState,
    pub(crate) capability: Option<crate::capabilities::PreparedCapabilityCandidate>,
    pub(crate) resources: Option<crate::runtime::resources::PreparedRuntimeResourceData>,
    pub(crate) impact: CacheImpact,
}

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
    Ready { impact: CacheImpact },
    Failed { diagnostic: String },
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
    pub candidate: Option<AvailableConfiguration>,
    pub eligibility: AdoptionEligibility,
}

#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct AvailableConfiguration {
    pub identity: ApplicationIdentity,
    pub expected_binding: u64,
    pub impact: CacheImpact,
}

struct ReadyConfiguration {
    capture: super::ProspectiveSessionConfig,
    prepared: Option<PreparedConfiguration>,
}
impl std::fmt::Debug for ReadyConfiguration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadyConfiguration").finish_non_exhaustive()
    }
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
    ready: BTreeMap<String, ReadyConfiguration>,
}

#[derive(Clone)]
pub(crate) struct CapturedApplication {
    pub(crate) policy: super::IndependentPolicy,
    pub(crate) process: super::super::app_server_policy::AppServerPolicy,
    pub(crate) revision: String,
    pub(crate) context: Result<super::ProspectiveSessionConfig, String>,
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

    pub(crate) fn lock(&self) -> MutexGuard<'_, ApplicationState> {
        self.inner.lock().expect("configuration application owner")
    }

    pub(crate) fn rebind_runtime(
        &self,
        scope: &str,
        runtime: &crate::runtime::conversation_runtime::ConversationRuntime,
    ) -> bool {
        let mut state = self.lock();
        let obsolete = state
            .ready
            .get(scope)
            .and_then(|ready| ready.prepared.as_ref())
            .is_some_and(|candidate| !runtime.configuration_allocation_matches(candidate));
        if !obsolete && !state.deferred.remove(scope) {
            return false;
        }
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
        state.ready.remove(scope);
        state.deferred.remove(scope);
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
    /// Initial resolution happens only for an uninitialized source scope. Once
    /// established, authored input cannot bypass successful native publication.
    pub(crate) fn initial_binding(
        &mut self,
        configuration: &super::UserConfigManager,
        input: &super::SessionConfigInput,
        credentials: &crate::credentials::CredentialSnapshot,
    ) -> Result<super::AdmittedSessionConfig, String> {
        let key = super::canonical_directory(&input.cwd)?;
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
                .unwrap_or_else(|| capture.config.initial_model().clone()),
        );
        Self::validate_selection(&capture, credentials)?;
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
            return Ok(std::borrow::Cow::Borrowed(capture));
        }
        let capture = configuration
            .resolve_session(&super::SessionConfigInput::new(key.to_path_buf()))
            .map_err(|error| error.to_string())?;
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
    ) -> Result<crate::model::catalog::ModelCatalogView, String> {
        let key = super::canonical_directory(cwd)?;
        let capture = self.creation_capture(configuration, &key, credentials)?;
        let catalog = capture
            .models
            .resolve(credentials)
            .map_err(|error| error.to_string())?;
        Ok(crate::model::invocation::ModelBindingRegistry::new(catalog)
            .map_err(|error| error.to_string())?
            .catalog_view())
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
                && context.same_context(adopted)
                && context.same_provider(adopted)
                && captured.policy.approval_mode == adopted.config.approval_mode
                && captured.policy.model_timeout_policy == adopted.config.model_timeout_policy
                && captured.policy.tool_deadline_policy == adopted.config.tool_deadline_policy
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
    ) -> Result<(), String> {
        capture
            .config
            .validate()
            .map_err(|error| error.to_string())?;
        for name in &capture.config.agent.workflows {
            if !capture.workflows.entries().contains_key(name) {
                return Err(format!("unknown Workflow {name}"));
            }
        }
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
        let model =
            crate::model::session::SessionModelState::new(models, capture.session_model().clone())
                .map_err(|error| error.to_string())?;
        let snapshot = model.snapshot();
        // The same native budget validator used at runtime composition.
        crate::runtime::conversation_runtime::validate_context_policy(
            &capture.config.context_policy(),
            &snapshot,
        )
        .map_err(|error| error.message)
    }

    fn validate_default(
        capture: &super::ProspectiveSessionConfig,
        credentials: &crate::credentials::CredentialSnapshot,
    ) -> Result<(), String> {
        let mut default = capture.clone();
        default.input.model = None;
        Self::validate_selection(&default, credentials)
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
                candidate: None,
                eligibility: AdoptionEligibility::Unavailable,
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
        self.ready.remove(&scope);
        self.pending.insert(scope, identity.clone());
        identity
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

    pub(crate) fn start_worker(&mut self) -> bool {
        if self.worker_running {
            return false;
        }
        self.worker_running = true;
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
        view
    }

    pub(crate) fn views(&self) -> Vec<ConfigurationApplication> {
        self.scopes
            .values()
            .cloned()
            .map(|view| self.owned(view))
            .collect()
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
        assert!(state.views().iter().all(|view| !view.sources.is_empty()));
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
        assert!(state.ready.is_empty());
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
            (
                ApplyUnit::Instructions,
                UnitApplication::Ready {
                    impact: CacheImpact::PrefixChanged,
                },
            ),
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
        assert!(matches!(
            view.units[&ApplyUnit::Instructions],
            UnitApplication::Ready { .. }
        ));
        assert!(matches!(
            view.units[&ApplyUnit::Capabilities],
            UnitApplication::Failed { .. }
        ));
    }
}
