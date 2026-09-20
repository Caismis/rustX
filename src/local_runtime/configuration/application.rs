//! Native application identity and publication serialization.
//!
//! This owner serializes source ingestion and final publication, not execution
//! admission or resource preparation. A single worker drains the latest desired
//! attempt; replacing pending work never makes stale work publishable again.
use std::collections::BTreeMap;
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
    pub scope: String,
    pub version: u64,
    pub desired: ApplicationIdentity,
    pub units: BTreeMap<ApplyUnit, UnitApplication>,
    pub candidate: Option<AvailableConfiguration>,
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
    next_attempt: u64,
    version: u64,
    scopes: BTreeMap<String, ConfigurationApplication>,
    pending: BTreeMap<String, ApplicationIdentity>,
    inputs: BTreeMap<String, Result<CapturedApplication, String>>,
    worker_running: bool,
    ready: BTreeMap<String, ReadyConfiguration>,
}

#[derive(Clone)]
pub(crate) struct CapturedApplication {
    pub(crate) policy: crate::local_runtime::config::CurrentRuntimeConfig,
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
        if !obsolete {
            return false;
        }
        let input = state.captured_input(scope);
        state.capture_binding(scope.to_owned(), input);
        self.notify(&state);
        true
    }

    pub(crate) fn forget(&self, scope: &str) {
        let mut state = self.lock();
        state.scopes.remove(scope);
        state.pending.remove(scope);
        state.inputs.remove(scope);
        state.ready.remove(scope);
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
        self.scopes.insert(
            scope.clone(),
            ConfigurationApplication {
                scope: scope.clone(),
                version: self.version,
                desired: identity.clone(),
                candidate: None,
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
        input: Result<CapturedApplication, String>,
    ) -> ApplicationIdentity {
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

    pub(crate) fn views(&self) -> Vec<ConfigurationApplication> {
        self.scopes.values().cloned().collect()
    }

    pub(crate) fn view(&self, scope: &str) -> Option<ConfigurationApplication> {
        self.scopes.get(scope).cloned()
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

    #[test]
    fn t08_failed_capture_has_no_manufactured_input_revision() {
        let mut state = ApplicationState::default();
        let identity = state.capture("s".into(), Err("input changed during capture".into()));
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
