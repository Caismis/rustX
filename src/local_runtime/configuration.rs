//! Reusable user-source ownership and fresh explicit-Session resolution.
//! No provider, Session runtime, or external source is prepared here.
//! Source content is reread per call; admitted configurations own their capture.

pub mod application;
pub(in crate::local_runtime) mod capture;
pub mod effective_observation;
pub mod generation;
pub mod observation;
mod prospective;
pub mod repair;
pub(in crate::local_runtime) mod semantics;
pub mod settings;
mod units;

use super::settings::{AdmittedSource, SourceReread};
use crate::bounded_file::read_bounded;
use crate::runtime::capability_inspection::{ResourceDefinition, ResourceFamily};
use crate::runtime::resources::SourceScope;
use std::collections::BTreeMap;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::authoring::RuntimeLayer;
use super::config::CurrentRuntimeConfig;
use super::diagnostics::LaunchFailure;
use crate::model::catalog::ModelCatalog;

/// Filesystem locations and controls produced by the launch resolver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLocations {
    /// The model-visible workspace root.
    pub workspace: PathBuf,
    /// The exact runtime-private root from which disjoint private
    /// subdirectories are derived. Child conversation stores live below its
    /// stable `subagents/<conversation-id>` semantic directory; child
    /// execution incarnations remain private and disposable.
    pub runtime_root: PathBuf,
}

impl SessionLocations {
    /// Prospective environment location; this does not grant storage authority.
    #[must_use]
    pub fn environment_store_root(&self) -> PathBuf {
        self.runtime_root.join("environments")
    }

    /// The capability environment store of one independent conversation
    /// lineage. Branches do not share mutable environment materialization.
    #[must_use]
    pub fn environment_store_root_for(
        &self,
        conversation_id: &crate::runtime::identity::ConversationId,
    ) -> PathBuf {
        self.environment_store_root().join(conversation_id.as_str())
    }
}

/// Stable user/process source bindings. No parsed effective configuration or credentials.
#[derive(Debug, Clone)]
pub struct UserConfigSources {
    pub home_directory: PathBuf,
    pub config_path: PathBuf,
    pub runtime_root: PathBuf,
}

/// Reusable source owner. Each resolution rereads current files; snapshots never
/// borrow mutable manager state. The manager has no process launch directory.
#[derive(Debug, Clone)]
pub struct UserConfigManager {
    pub(super) sources: UserConfigSources,
    runtime_root_origin: Origin,
    /// Process-local automatic repair observations, shared by every clone.
    repairs: repair::RepairLedger,
    #[cfg(test)]
    pub(crate) test_hooks: ConfigurationHooks,
}

#[cfg(test)]
type ConfigurationHookMap = BTreeMap<&'static str, Box<dyn FnOnce() + Send>>;
#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct ConfigurationHooks(
    std::sync::Arc<std::sync::Mutex<ConfigurationHookMap>>,
    std::sync::Arc<std::sync::Mutex<std::collections::BTreeSet<&'static str>>>,
);
#[cfg(test)]
impl std::fmt::Debug for ConfigurationHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ConfigurationHooks")
    }
}
#[cfg(test)]
impl ConfigurationHooks {
    pub(crate) fn insert(&self, point: &'static str, hook: impl FnOnce() + Send + 'static) {
        self.0.lock().unwrap().insert(point, Box::new(hook));
    }
    pub(crate) fn reach(&self, point: &'static str) {
        let hook = self.0.lock().unwrap().remove(point);
        if let Some(hook) = hook {
            hook();
        }
    }
    /// Make the next I/O step named `point` fail, once.
    pub(crate) fn fail_once(&self, point: &'static str) {
        self.1.lock().unwrap().insert(point);
    }
    /// Whether the I/O step named `point` is injected to fail now.
    pub(crate) fn fault(&self, point: &'static str) -> bool {
        self.1.lock().unwrap().remove(point)
    }
}

/// Intentional inputs for one prospective Session. Omission remains omission.
/// Paths are absolute; cwd is execution/configuration context, not a sandbox.
#[derive(Clone)]
pub struct SessionConfigInput {
    pub cwd: PathBuf,
    pub model: Option<crate::model::session::SessionModelConfig>,
}
impl SessionConfigInput {
    #[must_use]
    pub fn new(cwd: PathBuf) -> Self {
        Self { cwd, model: None }
    }
}

/// Values are never included: provenance cannot expose credentials or environment values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Origin {
    Builtin,
    User { document: PathBuf, base: PathBuf },
    Workspace { document: PathBuf, base: PathBuf },
    Process { base: PathBuf },
}

/// Validated Session configuration authority consumed by the existing native composition owner.
#[derive(Debug, Clone)]
pub struct AdmittedSessionConfig {
    pub(crate) binding_revision: u64,
    pub(crate) credentials: crate::credentials::CredentialSnapshot,
    prospective: Box<ProspectiveSessionConfig>,
}

/// Shared authority, source overlay and path capture, before domain validation.
#[derive(Clone)]
struct LayerCapture {
    revisions: BTreeMap<PathBuf, String>,
    locations: SessionLocations,
    identity: String,
    merged: RuntimeLayer,
    provenance: BTreeMap<String, Origin>,
    project_resources: Vec<PathBuf>,
}

/// Canonical source/configuration capture before any launch resource preparation.
pub(crate) struct SourceCapture {
    definitions: Vec<crate::runtime::capability_inspection::ResourceDefinition>,
    diagnostics: Vec<crate::runtime::capability_inspection::ResourceDiagnostic>,
    revisions: BTreeMap<PathBuf, String>,
    effective: RuntimeLayer,
    locations: SessionLocations,
    identity: String,
    pub(crate) config: CurrentRuntimeConfig,
    pub(crate) models: ModelCatalog,
    pub(crate) provenance: BTreeMap<String, Origin>,
    project_resources: Vec<PathBuf>,
}

/// Immutable prospective Session settings and statically resolved resources.
/// Admission adds credentials; native composition owns external preparation.
#[derive(Clone)]
pub struct ProspectiveSessionConfig {
    pub(crate) component_revisions: BTreeMap<application::ApplyUnit, String>,
    pub(crate) source_revisions: BTreeMap<PathBuf, String>,
    pub(crate) effective: RuntimeLayer,
    pub(crate) root_agent_project_files: Vec<crate::runtime::resources::ProjectContextFile>,
    pub(crate) skill_discovery: crate::skills::SkillDiscoveryOutcome,
    pub(crate) project_context_files: Vec<crate::runtime::resources::ProjectContextFile>,
    pub(crate) inspection: crate::runtime::capability_inspection::CapabilityInspection,
    pub(crate) input: SessionConfigInput,
    pub(crate) sources: UserConfigSources,
    pub(crate) managed_python: crate::runtime::resources::ManagedPythonCatalog,
    pub(crate) workflows: std::sync::Arc<crate::runtime::workflow::WorkflowCatalog>,
    pub(crate) subagents: crate::runtime::subagent::AgentCatalog,
    pub(crate) agent_root: PathBuf,
    pub(crate) role_sources:
        BTreeMap<crate::runtime::subagent::SubagentName, super::agent_resources::AgentSource>,
    pub(crate) locations: SessionLocations,
    pub(crate) config: std::sync::Arc<CurrentRuntimeConfig>,
    pub(crate) models: ModelCatalog,
    pub(crate) provenance: BTreeMap<String, Origin>,
    pub(crate) identity: String,
    pub(crate) skill_sources: Vec<crate::skills::AutomaticSkillRoot>,
    /// Project-origin paths retain their authority even after becoming absolute.
    project_resources: Vec<PathBuf>,
}

/// Captured authority for the two independent policy units, including when
/// resource resolution fails. Both source and runtime composition consume it.
#[derive(Clone)]
pub(crate) struct IndependentPolicy {
    pub(crate) config: CurrentRuntimeConfig,
    effective: RuntimeLayer,
    provenance: BTreeMap<String, Origin>,
    revision: String,
}
impl std::ops::Deref for IndependentPolicy {
    type Target = CurrentRuntimeConfig;
    fn deref(&self) -> &Self::Target {
        &self.config
    }
}

impl std::fmt::Debug for SessionConfigInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionConfigInput")
            .field("cwd", &self.cwd)
            .field("selections", &"<redacted>")
            .finish_non_exhaustive()
    }
}
