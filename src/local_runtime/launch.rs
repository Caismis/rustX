//! Local CLI input translation and immutable process bindings. Configuration semantics live
//! exclusively in `configuration::UserConfigManager`.
use super::composition::StartupSession;
use super::configuration::{
    AdmittedSessionConfig, ProspectiveSessionConfig, SessionConfigInput, SessionLocations,
    UserConfigManager, UserConfigSources, absolute, canonical_directory, present_on_disk,
};
use super::diagnostics::LaunchFailure;
use std::path::{Path, PathBuf};
/// Raw user intent. Absence is preserved until resolution.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchRequest {
    pub config: Option<PathBuf>,
    pub workspace: Option<PathBuf>,
    pub runtime_root: Option<PathBuf>,
    pub model: Option<String>,
    pub startup_session: StartupSession,
    pub session_name: Option<String>,
}

/// Captured once; tests supply isolated snapshots without changing process globals.
#[derive(Debug, Clone)]
pub struct HostEnvironment {
    pub launch_directory: PathBuf,
    /// The captured home directory.
    ///
    /// User resources remain `<home>/rustx/.agents` even when --config
    /// rebinds the User document. Runtime storage is a separate process binding.
    pub home_directory: PathBuf,
    pub config_directory: PathBuf,
    pub state_directory: PathBuf,
}

impl HostEnvironment {
    /// Capture the supported Unix host paths.
    ///
    /// # Errors
    /// Fails if the launch directory or required absolute host paths are unavailable.
    pub fn capture() -> Result<Self, String> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or("HOME is required to locate user configuration and resources")?;

        Self::from_paths(std::env::current_dir().map_err(|e| e.to_string())?, home)
    }

    /// Build an isolated host snapshot from explicit paths.
    ///
    /// # Errors
    /// Rejects relative HOME.
    #[allow(clippy::needless_pass_by_value)] // captured path inputs transfer together
    pub fn from_paths(launch_directory: PathBuf, home: PathBuf) -> Result<Self, String> {
        if !home.is_absolute() {
            return Err("HOME must be an absolute path".into());
        }
        Ok(Self {
            launch_directory,
            config_directory: home.join("rustx"),
            state_directory: home.join("rustx/runtime"),
            home_directory: home,
        })
    }
}

impl LaunchRequest {
    /// Translate CLI-relative spellings and discovery into explicit shared inputs.
    /// # Errors
    /// Invalid host paths or workspace discovery fail without effects.
    pub fn session_input(
        &self,
        host: &HostEnvironment,
    ) -> Result<(UserConfigManager, SessionConfigInput), LaunchFailure> {
        let (sources, input) = self.source_inputs(host)?;
        let launch = canonical_directory(&host.launch_directory)?;
        let manager = UserConfigManager::bootstrap(
            sources,
            self.runtime_root.as_ref().map(|p| absolute(&launch, p)),
        )?;
        Ok((manager, input))
    }

    fn source_inputs(
        &self,
        host: &HostEnvironment,
    ) -> Result<(UserConfigSources, SessionConfigInput), String> {
        for (flag, path) in [
            ("--config", self.config.as_ref()),
            ("--runtime-root", self.runtime_root.as_ref()),
        ] {
            if path.is_some_and(|path| !path.is_absolute()) {
                return Err(format!("{flag} requires an absolute process binding"));
            }
        }
        let launch = canonical_directory(&host.launch_directory)?;
        let cwd = match &self.workspace {
            Some(path) => canonical_directory(&absolute(&launch, path))?,
            None => discover_workspace(&launch)?,
        };
        let sources = UserConfigSources {
            home_directory: host.home_directory.clone(),
            config_path: self.config.as_ref().map_or_else(
                || host.config_directory.join("rustx.toml"),
                |p| absolute(&launch, p),
            ),
            runtime_root: host.state_directory.clone(),
        };
        let input = SessionConfigInput {
            cwd,
            model: self
                .model
                .as_ref()
                .map(|model| {
                    crate::model::catalog::ModelRef::parse(model)
                        .map(crate::model::session::SessionModelConfig::of)
                })
                .transpose()
                .map_err(|e| e.to_string())?,
        };
        Ok((sources, input))
    }
}
/// CLI projection of the shared static resolver.
/// # Errors
/// Returns the shared resolver's typed configuration failures.
pub fn analyze(
    request: &LaunchRequest,
    host: &HostEnvironment,
) -> Result<ProspectiveSessionConfig, LaunchFailure> {
    let (manager, input) = request.session_input(host)?;
    manager.resolve_session(&input)
}
/// CLI trust/inspection location input, without configuration reads.
/// # Errors
/// Invalid explicit locations are rejected.
pub fn resolve_locations(
    request: &LaunchRequest,
    host: &HostEnvironment,
) -> Result<(SessionLocations, String), String> {
    let (mut sources, input) = request.source_inputs(host)?;
    if let Some(root) = &request.runtime_root {
        sources.runtime_root = absolute(&host.launch_directory, root);
    }
    UserConfigManager::new(sources)?.resolve_locations(&input)
}
/// Resolve and admit CLI input through the shared Session configuration owner.
/// # Errors
/// Invalid configuration or resource authority prevents credential capture.
/// Workspace semantic units replace User units without trust gating.
pub fn resolve(
    request: &LaunchRequest,
    host: &HostEnvironment,
) -> Result<AdmittedSessionConfig, String> {
    analyze(request, host)?.admit(crate::credentials::CredentialSnapshot::capture)
}
/// Resolve inspection state without loading models or project configuration.
/// Only the host state-location member is relevant; runtime settings stay inert.
///
/// # Errors
/// Rejects invalid filesystem locations or an unreadable/malformed host state reference.
pub fn resolve_inspection_locations(
    request: &LaunchRequest,
    host: &HostEnvironment,
) -> Result<SessionLocations, String> {
    resolve_locations(request, host).map(|(locations, _)| locations)
}

pub(super) fn discover_workspace(launch: &Path) -> Result<PathBuf, String> {
    for (depth, directory) in launch.ancestors().enumerate() {
        if depth >= 128 {
            return Err("workspace discovery exceeded 128 directories; pass --workspace".into());
        }
        if present_on_disk(&directory.join(".git"))?
            || present_on_disk(&directory.join("rustx.toml"))?
        {
            return Ok(directory.into());
        }
    }
    Ok(launch.into())
}
