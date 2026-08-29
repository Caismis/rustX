//! Shared Python/Node environment identity and materialization (M6).
//!
//! # One shared environment per ecosystem
//!
//! For one active Skill capability set, all declared Python dependencies
//! materialize into **one** shared Python environment and all declared Node
//! dependencies into **one** shared Node environment — never one
//! environment per Skill. If the merged dependency set of an ecosystem is
//! empty, that ecosystem's runtime/package manager is not required and no
//! empty environment is materialized: the ecosystem environment is
//! represented as absent, so shell-only or Python-only Skill sets work
//! without Node and vice versa.
//!
//! # Environment identity
//!
//! [`python_environment_digest`] and [`node_environment_digest`] are
//! SHA-256 digests over a canonical deterministic input:
//!
//! - format/version domain separator;
//! - OS and architecture;
//! - resolved runtime version identity (`python3 --version`,
//!   `node --version`);
//! - resolved package-manager version identity (`pip`/`npm`);
//! - the sorted normalized direct dependency map.
//!
//! The digest never includes the Workspace absolute path, the environment
//! store path, the staging path, the current time, random values, or
//! `HashMap` iteration order. The same logical request under the same
//! environment/runtime inputs produces the same identity; different
//! environment-relevant inputs never alias to one mutable environment.
//! `SkillVersionId`, `PythonEnvironmentDigest`, and
//! `NodeEnvironmentDigest` remain distinct identities: a description-only
//! Skill change yields a new Skill version without changing environment
//! identities when dependency inputs are unchanged.
//!
//! # Runtime-private store
//!
//! Environments are materialized outside the model-visible Workspace in
//! one caller-configured runtime-private root, disjoint from the
//! Workspace:
//!
//! ```text
//! <skill-env-root>/
//! ├── python/
//! │   └── <digest>/
//! └── node/
//!     └── <digest>/
//! ```
//!
//! Environment-store paths are never model-visible in the Skill catalog.
//! Node publication is atomic rename from a same-filesystem private staging
//! directory:
//!
//! ```text
//! resolve candidate
//!     → create private staging directory (Node)
//!     → materialize
//!     → validate
//!     → write deterministic environment manifest/marker
//!     → atomic rename/publication (Node), or ready-marker commit (Python)
//!     → immutable digest directory
//! ```
//!
//! If an environment with the exact digest is already published, its
//! marker/manifest is validated against the expected digest inputs and the
//! environment is reused; nothing is ever installed into a published
//! environment again. Failed preparation removes its staging directory. No
//! environment GC framework is part of M6.
//!
//! # Package-manager subprocess ownership
//!
//! Every package-manager invocation (`python3 -m venv`, `pip install`,
//! `pip check`, `npm install`, `npm ls`, and the runtime version probes)
//! runs through the shared internal supervised command runner
//! (`crate::runtime::process_runner`): the same rustX-owned supervisor/
//! process-group domain, child-subreaper contract, cancellation/timeout
//! settlement, explicit cwd, explicit child environment, finite timeout,
//! and bounded diagnostics as native Bash. The runner-backed convenience
//! boundary owns each physical invocation in a runtime driver task: the
//! materializer/probe future waits for its result, but dropping that waiter
//! does not cancel or abandon a started subprocess. No independent bare
//! subprocess hierarchy exists.
//!
//! The materializer boundary is the [`SkillEnvironmentBackend`] seam:
//! production uses the runner-backed backend; deterministic tests inject
//! fakes and never touch a public package registry.
//!
//! Python environments are built directly at their final digest path because
//! venv-generated console scripts contain absolute interpreter paths. Python
//! contents remain uncommitted until the complete deterministic manifest is
//! atomically installed as the ready marker. A digest directory without that
//! marker is incomplete and is never reused.
//!
//! Candidate preparation callers wait on an `EnvironmentStore`-owned build
//! task for each `(ecosystem, digest)`. Dropping one caller only stops that
//! caller's wait; it never releases the in-flight entry or cancels the shared
//! physical materialization. The owner publishes the result and removes the
//! in-flight entry only after materialization, validation, and publication
//! have returned, so a retry cannot overlap the previous writer. Thus the
//! ownership layers are explicit: `EnvironmentStore` owns the logical build,
//! while the runtime process runner owns each physical subprocess invocation.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::BoxFuture;
use sha2::{Digest, Sha256};
use tokio::sync::Notify;

use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::identity::{NodeEnvironmentDigest, PythonEnvironmentDigest};
use crate::runtime::process_runner::{
    ProcessOutcomeIntent, SupervisedCommandSpec, SupervisedProcessRunner,
};
use crate::skills::dependencies::Ecosystem;

/// The format/version domain of the Python environment identity.
pub const PYTHON_ENVIRONMENT_FORMAT: &str = "rustx-python-environment-v1";
/// The format/version domain of the Node environment identity.
pub const NODE_ENVIRONMENT_FORMAT: &str = "rustx-node-environment-v1";
/// The deterministic environment manifest/marker file name.
pub const ENVIRONMENT_MANIFEST_FILE: &str = "RUSTX_ENV_MANIFEST.json";
/// The finite timeout of one materialization command.
pub const ENVIRONMENT_COMMAND_TIMEOUT: Duration = Duration::from_mins(10);
/// The finite timeout of one runtime version probe.
pub const RUNTIME_PROBE_TIMEOUT: Duration = Duration::from_mins(1);

/// The explicit PATH of materialization commands: the runtime-approved
/// baseline. Parent-process environment inheritance is never re-enabled.
const MATERIALIZATION_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// The resolved runtime identities of one ecosystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeVersions {
    /// The resolved runtime version identity (e.g. `Python 3.12.3`,
    /// `v22.1.0`).
    pub runtime: String,
    /// The resolved package-manager version identity (e.g.
    /// `pip 24.0 ...`, `10.2.3`).
    pub package_manager: String,
}

/// An environment identity/materialization failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvironmentPreparationError {
    /// The ecosystem's runtime or package manager is unavailable.
    RuntimeUnavailable {
        ecosystem: Ecosystem,
        detail: String,
    },
    /// The resolved runtime identity does not match the expected shape.
    InvalidRuntimeIdentity {
        ecosystem: Ecosystem,
        detail: String,
    },
    /// A published digest directory exists but its marker/manifest does not
    /// match the expected digest inputs; the published environment is never
    /// mutated and never reused.
    CorruptPublishedEnvironment {
        ecosystem: Ecosystem,
        digest: String,
        detail: String,
    },
    /// The staging directory could not be created.
    StagingFailed { detail: String },
    /// Materialization or validation failed.
    MaterializationFailed {
        ecosystem: Ecosystem,
        detail: String,
    },
    /// The atomic publication (rename) failed.
    PublicationFailed { detail: String },
}

impl core::fmt::Display for EnvironmentPreparationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::RuntimeUnavailable { ecosystem, detail } => {
                write!(f, "{ecosystem} runtime is unavailable: {detail}")
            }
            Self::InvalidRuntimeIdentity { ecosystem, detail } => {
                write!(f, "{ecosystem} runtime identity is invalid: {detail}")
            }
            Self::CorruptPublishedEnvironment {
                ecosystem,
                digest,
                detail,
            } => write!(
                f,
                "published {ecosystem} environment {digest} has an invalid manifest: {detail}"
            ),
            Self::StagingFailed { detail } => write!(f, "cannot create staging: {detail}"),
            Self::MaterializationFailed { ecosystem, detail } => {
                write!(
                    f,
                    "{ecosystem} environment materialization failed: {detail}"
                )
            }
            Self::PublicationFailed { detail } => {
                write!(f, "environment publication failed: {detail}")
            }
        }
    }
}

impl std::error::Error for EnvironmentPreparationError {}

/// The immutable shared Python environment of one capability set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PythonEnvironment {
    /// The deterministic environment identity.
    pub digest: PythonEnvironmentDigest,
    /// The published immutable environment root.
    pub root: PathBuf,
    /// `<root>/bin` (the PATH prefix and `VIRTUAL_ENV` root).
    pub bin_dir: PathBuf,
}

/// The immutable shared Node environment of one capability set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeEnvironment {
    /// The deterministic environment identity.
    pub digest: NodeEnvironmentDigest,
    /// The published immutable environment root.
    pub root: PathBuf,
    /// `<root>/node_modules/.bin` (the PATH prefix).
    pub bin_dir: PathBuf,
    /// `<root>/node_modules` (the `NODE_PATH` value).
    pub modules_dir: PathBuf,
}

/// The ecosystem materialization backend seam.
///
/// This is a real current boundary: production materialization consumes the
/// shared supervised process runner, and deterministic tests inject fakes
/// so no test ever touches `PyPI` or npm.
pub trait SkillEnvironmentBackend: Send + Sync {
    /// Resolves the runtime and package-manager version identities of one
    /// ecosystem.
    fn resolve_runtime_versions(
        &self,
        ecosystem: Ecosystem,
    ) -> BoxFuture<'_, Result<RuntimeVersions, String>>;

    /// Materializes the shared Python environment directly into its final
    /// digest directory:
    /// venv creation, exact-pin installation, and post-install validation.
    fn materialize_python<'a>(
        &'a self,
        environment_dir: &'a Path,
        dependencies: &'a BTreeMap<String, String>,
    ) -> BoxFuture<'a, Result<(), String>>;

    /// Materializes the shared Node environment into `staging`: private
    /// package.json, exact-pin installation, and post-install validation.
    fn materialize_node<'a>(
        &'a self,
        staging: &'a Path,
        dependencies: &'a BTreeMap<String, String>,
    ) -> BoxFuture<'a, Result<(), String>>;
}

/// The production backend: every materialization command runs through the
/// shared supervised command runner.
#[derive(Clone)]
pub struct RunnerBackedSkillEnvironmentBackend {
    runner: Arc<dyn crate::runtime::process_runner::SupervisedProcessRunner>,
}

impl RunnerBackedSkillEnvironmentBackend {
    /// A runner-backed materialization backend (internal construction; the
    /// capability coordinator wires the shared runner).
    #[must_use]
    pub(crate) fn new(
        runner: Arc<dyn crate::runtime::process_runner::SupervisedProcessRunner>,
    ) -> Self {
        Self { runner }
    }
}

impl SkillEnvironmentBackend for RunnerBackedSkillEnvironmentBackend {
    fn resolve_runtime_versions(
        &self,
        ecosystem: Ecosystem,
    ) -> BoxFuture<'_, Result<RuntimeVersions, String>> {
        Box::pin(async move {
            match ecosystem {
                Ecosystem::Python => {
                    let runtime =
                        probe(&self.runner, "python3 --version", RUNTIME_PROBE_TIMEOUT).await?;
                    let runtime = runtime.trim();
                    if !runtime.starts_with("Python ") || runtime.contains('\n') {
                        return Err(format!(
                            "python3 --version returned an unexpected identity {runtime:?}"
                        ));
                    }
                    let package_manager = probe(
                        &self.runner,
                        "python3 -m pip --version",
                        RUNTIME_PROBE_TIMEOUT,
                    )
                    .await?;
                    let package_manager = package_manager.trim();
                    if !package_manager.starts_with("pip ") || package_manager.contains('\n') {
                        return Err(format!(
                            "python3 -m pip --version returned an unexpected identity \
                             {package_manager:?}"
                        ));
                    }
                    Ok(RuntimeVersions {
                        runtime: runtime.to_owned(),
                        package_manager: package_manager.to_owned(),
                    })
                }
                Ecosystem::Node => {
                    let runtime =
                        probe(&self.runner, "node --version", RUNTIME_PROBE_TIMEOUT).await?;
                    let runtime = runtime.trim();
                    if runtime.is_empty() || runtime.contains('\n') {
                        return Err(format!(
                            "node --version returned an unexpected identity {runtime:?}"
                        ));
                    }
                    let package_manager =
                        probe(&self.runner, "npm --version", RUNTIME_PROBE_TIMEOUT).await?;
                    let package_manager = package_manager.trim();
                    if package_manager.is_empty() || package_manager.contains('\n') {
                        return Err(format!(
                            "npm --version returned an unexpected identity {package_manager:?}"
                        ));
                    }
                    Ok(RuntimeVersions {
                        runtime: runtime.to_owned(),
                        package_manager: package_manager.to_owned(),
                    })
                }
            }
        })
    }

    fn materialize_python<'a>(
        &'a self,
        environment_dir: &'a Path,
        dependencies: &'a BTreeMap<String, String>,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            // 1. Create the virtual environment at its final digest path.
            run_checked(
                &self.runner,
                format!("python3 -m venv {}", shell_quote(environment_dir)),
                environment_dir,
                ENVIRONMENT_COMMAND_TIMEOUT,
            )
            .await?;
            // 2. Install the exact direct pins with the final venv's own
            //    python (never a published environment).
            let venv_python = environment_dir.join("bin").join("python");
            let pins = dependencies
                .iter()
                .map(|(name, version)| {
                    format!("{}=={}", shell_quote_str(name), shell_quote_str(version))
                })
                .collect::<Vec<_>>()
                .join(" ");
            run_checked(
                &self.runner,
                format!(
                    "{} -m pip install --disable-pip-version-check --no-input {pins}",
                    shell_quote(&venv_python)
                ),
                environment_dir,
                ENVIRONMENT_COMMAND_TIMEOUT,
            )
            .await?;
            // 3. Validate the installation.
            run_checked(
                &self.runner,
                format!("{} -m pip check", shell_quote(&venv_python)),
                environment_dir,
                ENVIRONMENT_COMMAND_TIMEOUT,
            )
            .await?;
            Ok(())
        })
    }

    fn materialize_node<'a>(
        &'a self,
        staging: &'a Path,
        dependencies: &'a BTreeMap<String, String>,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            // 1. The deterministic private package.json with the sorted
            //    exact dependency map.
            let package_json = serde_json::json!({
                "name": "rustx-skill-environment",
                "private": true,
                "dependencies": dependencies,
            });
            std::fs::write(
                staging.join("package.json"),
                serde_json::to_string_pretty(&package_json)
                    .map_err(|error| format!("cannot serialize package.json: {error}"))?,
            )
            .map_err(|error| format!("cannot write package.json: {error}"))?;
            // 2. Install against the staging prefix. The cache stays inside
            //    the staging directory; audit/fund network work is disabled.
            run_checked(
                &self.runner,
                format!(
                    "npm install --prefix {} --no-audit --no-fund --cache {}/.npm-cache",
                    shell_quote(staging),
                    shell_quote(staging)
                ),
                staging,
                ENVIRONMENT_COMMAND_TIMEOUT,
            )
            .await?;
            // 3. Validate the resulting installation.
            run_checked(
                &self.runner,
                format!("npm ls --prefix {} --all", shell_quote(staging)),
                staging,
                ENVIRONMENT_COMMAND_TIMEOUT,
            )
            .await?;
            Ok(())
        })
    }
}

/// The runtime-private environment store of one capability owner.
#[derive(Clone)]
pub struct EnvironmentStore {
    root: PathBuf,
    backend: Arc<dyn SkillEnvironmentBackend>,
    next_staging: Arc<AtomicU64>,
    in_flight: Arc<Mutex<HashMap<BuildKey, Arc<BuildState>>>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BuildKey {
    ecosystem: Ecosystem,
    digest: String,
}

struct BuildState {
    result: Mutex<Option<Result<PathBuf, EnvironmentPreparationError>>>,
    notify: Notify,
}

/// Completes a process-local build entry if its EnvironmentStore-owned task
/// exits unexpectedly. The guard lives inside that detached owner task, never
/// in a candidate preparation caller, so caller cancellation cannot release
/// an in-flight build while its physical materialization is running.
struct BuildOwnerGuard {
    in_flight: Arc<Mutex<HashMap<BuildKey, Arc<BuildState>>>>,
    key: BuildKey,
    state: Arc<BuildState>,
    completed: bool,
}

impl BuildOwnerGuard {
    fn finish(&mut self, result: Result<PathBuf, EnvironmentPreparationError>) {
        *self
            .state
            .result
            .lock()
            .expect("environment build result lock") = Some(result);
        let mut in_flight = self.in_flight.lock().expect("environment build lock");
        if in_flight
            .get(&self.key)
            .is_some_and(|current| Arc::ptr_eq(current, &self.state))
        {
            in_flight.remove(&self.key);
        }
        self.completed = true;
        drop(in_flight);
        self.state.notify.notify_waiters();
    }
}

impl Drop for BuildOwnerGuard {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        self.finish(Err(EnvironmentPreparationError::MaterializationFailed {
            ecosystem: self.key.ecosystem,
            detail: "the environment build owner exited before terminal publication".to_owned(),
        }));
    }
}

impl EnvironmentStore {
    /// Creates the environment store at the caller-configured root.
    ///
    /// The root is created and canonicalized. Disjointness from the model
    /// Workspace is validated by the capability coordinator, which has both
    /// roots.
    ///
    /// # Errors
    ///
    /// Returns [`EnvironmentPreparationError::StagingFailed`] when the root
    /// cannot be created or canonicalized.
    #[allow(clippy::needless_pass_by_value)] // the root is consumed into the store
    pub fn new(
        root: PathBuf,
        backend: Arc<dyn SkillEnvironmentBackend>,
    ) -> Result<Self, EnvironmentPreparationError> {
        std::fs::create_dir_all(&root).map_err(|error| {
            EnvironmentPreparationError::StagingFailed {
                detail: format!(
                    "cannot create the environment store root {}: {error}",
                    root.display()
                ),
            }
        })?;
        let root = std::fs::canonicalize(&root).map_err(|error| {
            EnvironmentPreparationError::StagingFailed {
                detail: format!(
                    "cannot canonicalize the environment store root {}: {error}",
                    root.display()
                ),
            }
        })?;
        Ok(Self {
            root,
            backend,
            next_staging: Arc::new(AtomicU64::new(0)),
            in_flight: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// The canonical environment store root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Coalesces same-process builds of one immutable environment digest.
    /// Exactly one detached `EnvironmentStore` task owns materialization;
    /// equivalent callers only await its result and receive the same final
    /// environment path. A failed owner publishes no reusable state, and a
    /// later call can acquire ownership for a retry only after the owner has
    /// returned from the complete materialization lifecycle.
    async fn coordinate_build<F>(
        &self,
        key: BuildKey,
        build: F,
    ) -> Result<PathBuf, EnvironmentPreparationError>
    where
        F: Future<Output = Result<PathBuf, EnvironmentPreparationError>> + Send + 'static,
    {
        let (state, owner) = {
            let mut in_flight = self.in_flight.lock().expect("environment build lock");
            if let Some(state) = in_flight.get(&key) {
                (state.clone(), false)
            } else {
                let state = Arc::new(BuildState {
                    result: Mutex::new(None),
                    notify: Notify::new(),
                });
                in_flight.insert(key.clone(), state.clone());
                (state, true)
            }
        };
        if owner {
            let owner_guard = BuildOwnerGuard {
                in_flight: self.in_flight.clone(),
                key,
                state: state.clone(),
                completed: false,
            };
            // Dropping a JoinHandle detaches the task; it does not abort it.
            // The caller therefore cannot become the physical materialization
            // owner merely by being cancelled while waiting below.
            std::mem::drop(tokio::spawn(async move {
                let mut owner_guard = owner_guard;
                let result = build.await;
                owner_guard.finish(result);
            }));
        }

        let mut notified = Box::pin(state.notify.notified());
        loop {
            if let Some(result) = state
                .result
                .lock()
                .expect("environment build result lock")
                .clone()
            {
                return result;
            }
            notified.as_mut().enable();
            if state
                .result
                .lock()
                .expect("environment build result lock")
                .is_some()
            {
                continue;
            }
            notified.await;
            notified = Box::pin(state.notify.notified());
        }
    }

    /// Ensures the shared Python environment of the merged dependency set.
    ///
    /// Returns `None` when the ecosystem has no dependencies (no runtime,
    /// no package manager, and no environment are required).
    ///
    /// # Errors
    ///
    /// Returns [`EnvironmentPreparationError`] when the runtime is
    /// unavailable, materialization/validation fails, a published
    /// environment is corrupt, or publication fails.
    pub async fn ensure_python_environment(
        &self,
        dependencies: &BTreeMap<String, String>,
    ) -> Result<Option<PythonEnvironment>, EnvironmentPreparationError> {
        if dependencies.is_empty() {
            return Ok(None);
        }
        let versions = self
            .backend
            .resolve_runtime_versions(Ecosystem::Python)
            .await
            .map_err(|detail| EnvironmentPreparationError::RuntimeUnavailable {
                ecosystem: Ecosystem::Python,
                detail,
            })?;
        let digest = python_environment_digest(
            std::env::consts::OS,
            std::env::consts::ARCH,
            &versions.runtime,
            &versions.package_manager,
            dependencies,
        );
        let final_dir = self.root.join("python").join(digest.as_str());
        let build_key = BuildKey {
            ecosystem: Ecosystem::Python,
            digest: digest.to_string(),
        };
        let store = self.clone();
        let build_versions = versions.clone();
        let build_dependencies = dependencies.clone();
        let build_digest = digest.to_string();
        let published = self
            .coordinate_build(build_key, async move {
                store
                    .build_python_environment(
                        final_dir,
                        &build_digest,
                        &build_versions,
                        &build_dependencies,
                    )
                    .await
            })
            .await?;
        validate_published_manifest(
            &published,
            PYTHON_ENVIRONMENT_FORMAT,
            digest.as_str(),
            std::env::consts::OS,
            std::env::consts::ARCH,
            &versions.runtime,
            &versions.package_manager,
            dependencies,
        )
        .map_err(
            |detail| EnvironmentPreparationError::CorruptPublishedEnvironment {
                ecosystem: Ecosystem::Python,
                digest: digest.to_string(),
                detail,
            },
        )?;
        Ok(Some(PythonEnvironment {
            bin_dir: published.join("bin"),
            root: published,
            digest,
        }))
    }

    /// Ensures the shared Node environment of the merged dependency set.
    ///
    /// Returns `None` when the ecosystem has no dependencies.
    ///
    /// # Errors
    ///
    /// See [`EnvironmentStore::ensure_python_environment`].
    pub async fn ensure_node_environment(
        &self,
        dependencies: &BTreeMap<String, String>,
    ) -> Result<Option<NodeEnvironment>, EnvironmentPreparationError> {
        if dependencies.is_empty() {
            return Ok(None);
        }
        let versions = self
            .backend
            .resolve_runtime_versions(Ecosystem::Node)
            .await
            .map_err(|detail| EnvironmentPreparationError::RuntimeUnavailable {
                ecosystem: Ecosystem::Node,
                detail,
            })?;
        let digest = node_environment_digest(
            std::env::consts::OS,
            std::env::consts::ARCH,
            &versions.runtime,
            &versions.package_manager,
            dependencies,
        );
        let final_dir = self.root.join("node").join(digest.as_str());
        let build_key = BuildKey {
            ecosystem: Ecosystem::Node,
            digest: digest.to_string(),
        };
        let store = self.clone();
        let build_versions = versions.clone();
        let build_dependencies = dependencies.clone();
        let build_digest = digest.to_string();
        let published = self
            .coordinate_build(build_key, async move {
                store
                    .build_node_environment(
                        final_dir,
                        &build_digest,
                        &build_versions,
                        &build_dependencies,
                    )
                    .await
            })
            .await?;
        validate_published_manifest(
            &published,
            NODE_ENVIRONMENT_FORMAT,
            digest.as_str(),
            std::env::consts::OS,
            std::env::consts::ARCH,
            &versions.runtime,
            &versions.package_manager,
            dependencies,
        )
        .map_err(
            |detail| EnvironmentPreparationError::CorruptPublishedEnvironment {
                ecosystem: Ecosystem::Node,
                digest: digest.to_string(),
                detail,
            },
        )?;
        Ok(Some(NodeEnvironment {
            modules_dir: published.join("node_modules"),
            bin_dir: published.join("node_modules").join(".bin"),
            root: published,
            digest,
        }))
    }

    async fn build_python_environment(
        &self,
        final_dir: PathBuf,
        digest: &str,
        versions: &RuntimeVersions,
        dependencies: &BTreeMap<String, String>,
    ) -> Result<PathBuf, EnvironmentPreparationError> {
        if final_dir.join(ENVIRONMENT_MANIFEST_FILE).exists() {
            validate_published_manifest(
                &final_dir,
                PYTHON_ENVIRONMENT_FORMAT,
                digest,
                std::env::consts::OS,
                std::env::consts::ARCH,
                &versions.runtime,
                &versions.package_manager,
                dependencies,
            )
            .map_err(|detail| {
                EnvironmentPreparationError::CorruptPublishedEnvironment {
                    ecosystem: Ecosystem::Python,
                    digest: digest.to_owned(),
                    detail,
                }
            })?;
            return Ok(final_dir);
        }
        remove_incomplete_environment(&final_dir)?;
        create_staging(&final_dir)?;
        if let Err(detail) = self
            .backend
            .materialize_python(&final_dir, dependencies)
            .await
        {
            let _ = std::fs::remove_dir_all(&final_dir);
            return Err(EnvironmentPreparationError::MaterializationFailed {
                ecosystem: Ecosystem::Python,
                detail,
            });
        }
        validate_python_materialization(&final_dir).map_err(|detail| {
            let _ = std::fs::remove_dir_all(&final_dir);
            EnvironmentPreparationError::MaterializationFailed {
                ecosystem: Ecosystem::Python,
                detail,
            }
        })?;
        if let Err(detail) = write_manifest_atomic(
            &final_dir,
            PYTHON_ENVIRONMENT_FORMAT,
            digest,
            std::env::consts::OS,
            std::env::consts::ARCH,
            &versions.runtime,
            &versions.package_manager,
            dependencies,
        ) {
            let _ = std::fs::remove_dir_all(&final_dir);
            return Err(EnvironmentPreparationError::PublicationFailed { detail });
        }
        Ok(final_dir)
    }

    async fn build_node_environment(
        &self,
        final_dir: PathBuf,
        digest: &str,
        versions: &RuntimeVersions,
        dependencies: &BTreeMap<String, String>,
    ) -> Result<PathBuf, EnvironmentPreparationError> {
        if final_dir.join(ENVIRONMENT_MANIFEST_FILE).exists() {
            validate_published_manifest(
                &final_dir,
                NODE_ENVIRONMENT_FORMAT,
                digest,
                std::env::consts::OS,
                std::env::consts::ARCH,
                &versions.runtime,
                &versions.package_manager,
                dependencies,
            )
            .map_err(|detail| {
                EnvironmentPreparationError::CorruptPublishedEnvironment {
                    ecosystem: Ecosystem::Node,
                    digest: digest.to_owned(),
                    detail,
                }
            })?;
            return Ok(final_dir);
        }
        remove_incomplete_environment(&final_dir)?;
        let staging = self.staging_dir("node");
        create_staging(&staging)?;
        if let Err(detail) = self.backend.materialize_node(&staging, dependencies).await {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(EnvironmentPreparationError::MaterializationFailed {
                ecosystem: Ecosystem::Node,
                detail,
            });
        }
        if let Err(detail) = write_manifest(
            &staging,
            NODE_ENVIRONMENT_FORMAT,
            digest,
            std::env::consts::OS,
            std::env::consts::ARCH,
            &versions.runtime,
            &versions.package_manager,
            dependencies,
        ) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(EnvironmentPreparationError::PublicationFailed { detail });
        }
        publish(staging, &final_dir)?;
        Ok(final_dir)
    }

    /// The deterministic private staging directory of one ecosystem:
    /// process-unique and sequence-unique, on the same filesystem as the
    /// final target. The staging name never enters any digest.
    fn staging_dir(&self, ecosystem: &str) -> PathBuf {
        let sequence = self.next_staging.fetch_add(1, Ordering::SeqCst);
        self.root
            .join(ecosystem)
            .join(format!(".staging-{}-{sequence}", std::process::id()))
    }
}

/// Creates the staging directory (and its ecosystem parent).
fn create_staging(staging: &Path) -> Result<(), EnvironmentPreparationError> {
    std::fs::create_dir_all(staging).map_err(|error| EnvironmentPreparationError::StagingFailed {
        detail: format!(
            "cannot create staging directory {}: {error}",
            staging.display()
        ),
    })
}

/// Removes an uncommitted digest directory. A directory with a manifest is
/// intentionally left untouched so a corrupt committed environment is
/// reported rather than silently replaced.
fn remove_incomplete_environment(path: &Path) -> Result<(), EnvironmentPreparationError> {
    if !path.exists() {
        return Ok(());
    }
    if path.join(ENVIRONMENT_MANIFEST_FILE).exists() {
        return Ok(());
    }
    std::fs::remove_dir_all(path).map_err(|error| EnvironmentPreparationError::StagingFailed {
        detail: format!(
            "cannot remove incomplete environment {}: {error}",
            path.display()
        ),
    })
}

/// Validates the minimum Python publication shape before the ready marker is
/// committed. The backend performs ecosystem-specific package validation;
/// this boundary ensures the PATH advertised by the capability has a real
/// final-path `bin` directory.
fn validate_python_materialization(path: &Path) -> Result<(), String> {
    if !path.join("bin").is_dir() {
        return Err(format!(
            "Python materialization did not create its final bin directory: {}",
            path.display()
        ));
    }
    Ok(())
}

/// Publishes the materialized staging directory atomically.
#[allow(clippy::needless_pass_by_value)] // the staging directory is consumed by the rename
fn publish(staging: PathBuf, final_dir: &Path) -> Result<(), EnvironmentPreparationError> {
    std::fs::create_dir_all(final_dir.parent().expect("ecosystem parent")).map_err(|error| {
        EnvironmentPreparationError::PublicationFailed {
            detail: format!("cannot create the ecosystem directory: {error}"),
        }
    })?;
    match std::fs::rename(&staging, final_dir) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging);
            Err(EnvironmentPreparationError::PublicationFailed {
                detail: format!(
                    "cannot rename {} into {}: {error}",
                    staging.display(),
                    final_dir.display()
                ),
            })
        }
    }
}

/// The deterministic environment manifest written before publication and
/// validated on reuse.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
struct EnvironmentManifest {
    format: String,
    digest: String,
    os: String,
    arch: String,
    runtime_version: String,
    package_manager_version: String,
    dependencies: BTreeMap<String, String>,
}

/// Writes the deterministic environment manifest into the staging
/// directory.
#[allow(clippy::too_many_arguments)] // one deterministic manifest boundary
fn write_manifest(
    staging: &Path,
    format: &str,
    digest: &str,
    os: &str,
    arch: &str,
    runtime_version: &str,
    package_manager_version: &str,
    dependencies: &BTreeMap<String, String>,
) -> Result<(), String> {
    let manifest = EnvironmentManifest {
        format: format.to_owned(),
        digest: digest.to_owned(),
        os: os.to_owned(),
        arch: arch.to_owned(),
        runtime_version: runtime_version.to_owned(),
        package_manager_version: package_manager_version.to_owned(),
        dependencies: dependencies.clone(),
    };
    let bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| format!("cannot serialize the environment manifest: {error}"))?;
    std::fs::write(staging.join(ENVIRONMENT_MANIFEST_FILE), bytes)
        .map_err(|error| format!("cannot write the environment manifest: {error}"))
}

/// Atomically commits the Python manifest as the ready marker. The digest
/// directory may contain incomplete contents before this rename, but it is
/// not reusable until this boundary succeeds.
#[allow(clippy::too_many_arguments)]
fn write_manifest_atomic(
    directory: &Path,
    format: &str,
    digest: &str,
    os: &str,
    arch: &str,
    runtime_version: &str,
    package_manager_version: &str,
    dependencies: &BTreeMap<String, String>,
) -> Result<(), String> {
    let manifest = EnvironmentManifest {
        format: format.to_owned(),
        digest: digest.to_owned(),
        os: os.to_owned(),
        arch: arch.to_owned(),
        runtime_version: runtime_version.to_owned(),
        package_manager_version: package_manager_version.to_owned(),
        dependencies: dependencies.clone(),
    };
    let bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| format!("cannot serialize the environment manifest: {error}"))?;
    let temporary = directory.join(format!("{ENVIRONMENT_MANIFEST_FILE}.tmp"));
    std::fs::write(&temporary, bytes)
        .map_err(|error| format!("cannot write the environment ready marker: {error}"))?;
    std::fs::rename(&temporary, directory.join(ENVIRONMENT_MANIFEST_FILE)).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        format!("cannot commit the environment ready marker: {error}")
    })
}

/// Validates a published environment's manifest against the expected
/// digest inputs. A published environment is never modified; a manifest
/// mismatch means the digest directory is not trustworthy and must not be
/// reused.
#[allow(clippy::too_many_arguments)] // one deterministic manifest boundary
fn validate_published_manifest(
    final_dir: &Path,
    format: &str,
    digest: &str,
    os: &str,
    arch: &str,
    runtime_version: &str,
    package_manager_version: &str,
    dependencies: &BTreeMap<String, String>,
) -> Result<(), String> {
    let bytes = std::fs::read(final_dir.join(ENVIRONMENT_MANIFEST_FILE)).map_err(|error| {
        format!(
            "the published environment has no readable manifest {}: {error}",
            final_dir.display()
        )
    })?;
    let manifest: EnvironmentManifest = serde_json::from_slice(&bytes)
        .map_err(|error| format!("the published environment manifest is invalid: {error}"))?;
    let expected = EnvironmentManifest {
        format: format.to_owned(),
        digest: digest.to_owned(),
        os: os.to_owned(),
        arch: arch.to_owned(),
        runtime_version: runtime_version.to_owned(),
        package_manager_version: package_manager_version.to_owned(),
        dependencies: dependencies.clone(),
    };
    if manifest != expected {
        return Err("the published manifest does not match the expected digest inputs".to_owned());
    }
    Ok(())
}

/// Runs one supervised command and returns its bounded combined output.
async fn probe(
    runner: &Arc<dyn SupervisedProcessRunner>,
    command: &str,
    timeout: Duration,
) -> Result<String, String> {
    let result = runner
        .run(
            SupervisedCommandSpec {
                command: command.to_owned(),
                cwd: std::env::temp_dir(),
                environment: materialization_environment(&std::env::temp_dir()),
                timeout: Some(timeout),
                cancellation: CancellationSignal::new(),
            },
            None,
        )
        .await
        .map_err(|error| format!("cannot run {command}: {error}"))?;
    match result.intent {
        ProcessOutcomeIntent::Completed => {
            if result.exit_code != Some(0) {
                return Err(format!("{command} exited with code {:?}", result.exit_code));
            }
            let mut output = result.stdout;
            output.extend_from_slice(&result.stderr);
            Ok(String::from_utf8_lossy(&output).into_owned())
        }
        ProcessOutcomeIntent::Cancelled => Err(format!("{command} was cancelled")),
        ProcessOutcomeIntent::TimedOut => Err(format!("{command} timed out")),
        ProcessOutcomeIntent::ProcessControlFailed(error) => {
            Err(format!("{command} failed: {error}"))
        }
    }
}

/// Runs one supervised materialization command; any non-zero exit or
/// process-control failure fails the materialization.
async fn run_checked(
    runner: &Arc<dyn SupervisedProcessRunner>,
    command: String,
    cwd: &Path,
    timeout: Duration,
) -> Result<(), String> {
    let result = runner
        .run(
            SupervisedCommandSpec {
                command,
                cwd: cwd.to_path_buf(),
                environment: materialization_environment(cwd),
                timeout: Some(timeout),
                cancellation: CancellationSignal::new(),
            },
            None,
        )
        .await
        .map_err(|error| format!("cannot run the materialization command: {error}"))?;
    match result.intent {
        ProcessOutcomeIntent::Completed => {
            if result.exit_code == Some(0) {
                Ok(())
            } else {
                Err(format!(
                    "command exited with code {:?}; stderr: {}",
                    result.exit_code,
                    String::from_utf8_lossy(&result.stderr)
                ))
            }
        }
        ProcessOutcomeIntent::Cancelled => {
            Err("the materialization command was cancelled".to_owned())
        }
        ProcessOutcomeIntent::TimedOut => Err("the materialization command timed out".to_owned()),
        ProcessOutcomeIntent::ProcessControlFailed(error) => {
            Err(format!("the materialization command failed: {error}"))
        }
    }
}

/// The explicit child environment of every materialization command:
/// `env_clear()` semantics with the runtime-approved PATH and a
/// staging-scoped HOME. Parent-process environment inheritance is never
/// re-enabled.
fn materialization_environment(cwd: &Path) -> Vec<(String, String)> {
    vec![
        ("PATH".to_owned(), MATERIALIZATION_PATH.to_owned()),
        ("HOME".to_owned(), cwd.display().to_string()),
        ("LANG".to_owned(), "C.UTF-8".to_owned()),
        ("LC_ALL".to_owned(), "C.UTF-8".to_owned()),
    ]
}

/// Shell-quotes one path for the owned `/bin/bash -c` command.
fn shell_quote(value: &Path) -> String {
    shell_quote_str(&value.display().to_string())
}

/// Shell-quotes one argument for the owned `/bin/bash -c` command.
fn shell_quote_str(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Computes the deterministic Python environment digest.
///
/// # Panics
///
/// Panics only if a runtime/package-manager identity contains a newline or
/// `=` — callers validate resolved identities before computing the digest.
#[must_use]
pub fn python_environment_digest(
    os: &str,
    arch: &str,
    runtime_version: &str,
    package_manager_version: &str,
    dependencies: &BTreeMap<String, String>,
) -> PythonEnvironmentDigest {
    environment_digest(
        PYTHON_ENVIRONMENT_FORMAT,
        os,
        arch,
        runtime_version,
        package_manager_version,
        dependencies,
    )
    .into()
}

/// Computes the deterministic Node environment digest.
///
/// # Panics
///
/// Panics only if a runtime/package-manager identity contains a newline or
/// `=` — callers validate resolved identities before computing the digest.
#[must_use]
pub fn node_environment_digest(
    os: &str,
    arch: &str,
    runtime_version: &str,
    package_manager_version: &str,
    dependencies: &BTreeMap<String, String>,
) -> NodeEnvironmentDigest {
    environment_digest(
        NODE_ENVIRONMENT_FORMAT,
        os,
        arch,
        runtime_version,
        package_manager_version,
        dependencies,
    )
    .into()
}

/// The shared canonical digest computation over the environment-relevant
/// inputs. The digest never includes workspace/store/staging paths, time,
/// random values, or `HashMap` iteration order.
fn environment_digest(
    format: &str,
    os: &str,
    arch: &str,
    runtime_version: &str,
    package_manager_version: &str,
    dependencies: &BTreeMap<String, String>,
) -> String {
    assert!(
        !runtime_version.contains('\n') && !runtime_version.contains('='),
        "runtime identity must be single-line and '='-free"
    );
    assert!(
        !package_manager_version.contains('\n') && !package_manager_version.contains('='),
        "package-manager identity must be single-line and '='-free"
    );
    let mut bytes = Vec::new();
    bytes.extend_from_slice(format!("{format}\n").as_bytes());
    bytes.extend_from_slice(format!("os={os}\n").as_bytes());
    bytes.extend_from_slice(format!("arch={arch}\n").as_bytes());
    bytes.extend_from_slice(format!("runtime={runtime_version}\n").as_bytes());
    bytes.extend_from_slice(format!("package-manager={package_manager_version}\n").as_bytes());
    for (name, version) in dependencies {
        bytes.extend_from_slice(format!("dependency={name}={version}\n").as_bytes());
    }
    let digest = Sha256::digest(&bytes);
    let mut hex = String::with_capacity(71);
    hex.push_str("sha256:");
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

impl From<String> for PythonEnvironmentDigest {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<String> for NodeEnvironmentDigest {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    use crate::runtime::process_runner::{
        CapturedProcessResult, ProcessOutcomeIntent, RunnerBackedProcessRunner, RunnerTestControl,
    };

    struct ScriptedRunner {
        results: Mutex<VecDeque<Result<CapturedProcessResult, String>>>,
    }

    impl SupervisedProcessRunner for ScriptedRunner {
        fn run(
            &self,
            _spec: SupervisedCommandSpec,
            _control: Option<crate::runtime::process_runner::RunnerTestControl>,
        ) -> BoxFuture<'_, Result<CapturedProcessResult, String>> {
            let result = self
                .results
                .lock()
                .expect("scripted probe result lock")
                .pop_front()
                .expect("scripted probe result");
            Box::pin(async move { result })
        }
    }

    #[tokio::test]
    async fn runtime_probe_rejects_completed_nonzero_process() {
        let runner: Arc<dyn SupervisedProcessRunner> = Arc::new(ScriptedRunner {
            results: Mutex::new(VecDeque::from([Ok(CapturedProcessResult {
                exit_code: Some(1),
                intent: ProcessOutcomeIntent::Completed,
                stdout: b"Python 3.12.0\n".to_vec(),
                stderr: Vec::new(),
            })])),
        });
        let backend = RunnerBackedSkillEnvironmentBackend::new(runner);

        let error = backend
            .resolve_runtime_versions(Ecosystem::Python)
            .await
            .expect_err("a non-zero probe cannot publish a runtime identity");

        assert!(error.contains("python3 --version exited with code Some(1)"));
    }

    /// A real capability preparation reaches the production runner-backed
    /// Python probe. Once that probe has established ownership and reported
    /// shell exit, aborting the preparation waiter must not drop the physical
    /// runner before its terminal event and direct-child reap.
    #[cfg(unix)]
    #[tokio::test]
    async fn dropped_prepare_candidate_waiter_does_not_abandon_runtime_probe() {
        let dir = tempfile::tempdir().expect("temporary test directory");
        let workspace_root = dir.path().join("workspace");
        let skill_dir = workspace_root.join(".agents/skills/python");
        std::fs::create_dir_all(&skill_dir).expect("skill directory");
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: python\ndescription: probe ownership\nmetadata:\n  rustx.python-dependencies: '{\"example\":\"1.0.0\"}'\n---\nprobe\n",
        )
        .expect("skill manifest");
        let workspace =
            crate::tools::workspace::Workspace::new(&workspace_root).expect("workspace");
        let conversation_id = crate::runtime::identity::ConversationId::new("probe-drop");
        let mut control = RunnerTestControl::new();
        control.pause_at_shell_exit = true;
        let hook = control.lifecycle.clone();
        let runner = Arc::new(RunnerBackedProcessRunner::with_test_control(control));
        let backend = Arc::new(RunnerBackedSkillEnvironmentBackend::new(runner));
        let coordinator = crate::capabilities::CapabilityCoordinator::with_backend(
            crate::capabilities::CapabilityCoordinatorConfig {
                conversation_id,
                workspace: workspace.clone(),
                base_tool_registry: Arc::new(crate::tools::executor::ToolRegistry::new()),
                tool_activation: crate::capabilities::ToolActivationPolicy::default(),
                skill_discovery: crate::skills::SkillDiscoveryConfig::default_for_workspace(
                    &workspace,
                ),
                mcp_servers: std::collections::BTreeMap::new(),
                base_environment: crate::tools::environment::ToolEnvironment::new(),
                environment_store_root: dir.path().join("skill-env"),
                python_store_roots: None,
            },
            backend,
        )
        .expect("capability coordinator");
        let prepare = tokio::spawn(async move { coordinator.prepare_candidate().await });

        tokio::time::timeout(Duration::from_secs(15), hook.await_ownership_established())
            .await
            .expect("the runtime probe must establish physical ownership");
        tokio::time::timeout(Duration::from_secs(15), hook.await_shell_exit())
            .await
            .expect("the probe must reach the controlled pre-settlement boundary");
        prepare.abort();
        assert!(
            prepare
                .await
                .expect_err("the preparation waiter must be aborted")
                .is_cancelled(),
            "caller cancellation only drops the preparation waiter"
        );

        hook.release();
        tokio::time::timeout(Duration::from_secs(15), hook.await_terminal())
            .await
            .expect("the detached probe driver must prove group terminality");
        tokio::time::timeout(Duration::from_secs(15), hook.await_direct_child_reaped())
            .await
            .expect("the detached probe driver must reap the supervisor child");
    }
}
